use anyhow::{anyhow, Context, Result};
use gimli::{
    AttributeValue, DebuggingInformationEntry, Dwarf, EndianSlice, Reader, RunTimeEndian, Unit,
};
use object::{Object, ObjectSection};
use std::env;
use std::fs::File;
use std::path::Path;

struct FunctionStats {
    name: String,
    total_size: u64,
    self_size: u64,
    inlined_count: usize,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <elf-file>", args[0]);
        std::process::exit(1);
    }

    let path = Path::new(&args[1]);
    let file = File::open(path).with_context(|| format!("Failed to open file: {:?}", path))?;
    let mmap = unsafe { memmap2::Mmap::map(&file)? };
    let object = object::File::parse(&*mmap)?;

    let endian = if object.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };

    let load_section = |id: gimli::SectionId| -> Result<EndianSlice<RunTimeEndian>> {
        let name = id.name();
        match object.section_by_name(name) {
            Some(section) => Ok(EndianSlice::new(section.data()?, endian)),
            None => Ok(EndianSlice::new(&[], endian)),
        }
    };

    let dwarf = Dwarf::load(&load_section)?;
    let mut units = dwarf.units();

    println!(
        "{:>10} {:>10} {:>8}   {}",
        "# Total Sz", "Self Sz", "Inlines", "Function Name"
    );

    while let Some(header) = units.next()? {
        let comp_unit = dwarf.unit(header)?;
        let mut entries = comp_unit.entries();
        while let Some(entry) = entries
            .next_dfs()
            .map_err(|e| anyhow!("DWARF error: {}", e))?
        {
            if entry.tag() == gimli::DW_TAG_subprogram {
                if let Some(stats) = analyze_function(&dwarf, &comp_unit, entry)? {
                    println!(
                        "{:>10} {:>10} {:>8}   {}",
                        stats.total_size, stats.self_size, stats.inlined_count, stats.name
                    );
                }
            }
        }
    }

    Ok(())
}

fn analyze_function<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
) -> Result<Option<FunctionStats>> {
    // Get function name
    let name = resolve_name(dwarf, unit, entry)?;

    let total_size = get_entry_size(dwarf, unit, entry)?;
    if total_size == 0 {
        return Ok(None);
    }

    // Analyze children for inlined functions
    let mut tree = unit
        .entries_tree(Some(entry.offset()))
        .map_err(|e| anyhow!("{}", e))?;
    let root = tree.root().map_err(|e| anyhow!("{}", e))?;

    let (inlined_size, inlined_count) = collect_inlines_in_child(dwarf, unit, root)?;

    let mut self_size = 0;
    if total_size > inlined_size {
        self_size = total_size - inlined_size;
    }

    Ok(Some(FunctionStats {
        name,
        total_size,
        self_size,
        inlined_count,
    }))
}

fn resolve_name<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
) -> Result<String> {
    let name = if let Some(name_attr) = entry.attr_value(gimli::DW_AT_name) {
        dwarf
            .attr_string(unit, name_attr)
            .map_err(|e| anyhow!("{}", e))?
            .to_string_lossy()
            .map_err(|e| anyhow!("{}", e))?
            .into_owned()
    } else if let Some(origin) = entry.attr_value(gimli::DW_AT_abstract_origin) {
        resolve_name_ref(dwarf, unit, origin)?
    } else if let Some(spec) = entry.attr_value(gimli::DW_AT_specification) {
        resolve_name_ref(dwarf, unit, spec)?
    } else {
        "unknown".to_string()
    };

    Ok(name)
}

fn resolve_name_ref<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    attr: AttributeValue<R>,
) -> Result<String> {
    match attr {
        AttributeValue::UnitRef(offset) => {
            let entry = unit.entry(offset).map_err(|e| anyhow!("{}", e))?;
            if let Some(name_attr) = entry.attr_value(gimli::DW_AT_name) {
                Ok(dwarf
                    .attr_string(unit, name_attr)
                    .map_err(|e| anyhow!("{}", e))?
                    .to_string_lossy()
                    .map_err(|e| anyhow!("{}", e))?
                    .into_owned())
            } else if let Some(origin) = entry.attr_value(gimli::DW_AT_abstract_origin) {
                resolve_name_ref(dwarf, unit, origin)
            } else if let Some(spec) = entry.attr_value(gimli::DW_AT_specification) {
                resolve_name_ref(dwarf, unit, spec)
            } else {
                Ok("unknown".to_string())
            }
        }
        AttributeValue::DebugInfoRef(offset) => {
            let header = dwarf
                .debug_info
                .header_from_offset(offset)
                .map_err(|e| anyhow!("{}", e))?;
            let unit = dwarf.unit(header).map_err(|e| anyhow!("{}", e))?;
            let entry = unit
                .entry(
                    offset
                        .to_unit_offset(&unit.header)
                        .ok_or_else(|| anyhow!("Invalid offset"))?,
                )
                .map_err(|e| anyhow!("{}", e))?;
            if let Some(name_attr) = entry.attr_value(gimli::DW_AT_name) {
                Ok(dwarf
                    .attr_string(&unit, name_attr)
                    .map_err(|e| anyhow!("{}", e))?
                    .to_string_lossy()
                    .map_err(|e| anyhow!("{}", e))?
                    .into_owned())
            } else {
                Ok("unknown".to_string())
            }
        }
        _ => Ok("unknown".to_string()),
    }
}

fn get_entry_size<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
) -> Result<u64> {
    let mut size = 0;
    if let Some(low_pc_attr) = entry.attr_value(gimli::DW_AT_low_pc) {
        let low_pc = match low_pc_attr {
            AttributeValue::Addr(addr) => addr,
            _ => 0,
        };
        if let Some(high_pc_attr) = entry.attr_value(gimli::DW_AT_high_pc) {
            match high_pc_attr {
                AttributeValue::Addr(addr) => {
                    size = addr.saturating_sub(low_pc);
                }
                AttributeValue::Udata(s) => {
                    size = s;
                }
                _ => {}
            }
        }
    } else if let Some(ranges_attr) = entry.attr_value(gimli::DW_AT_ranges) {
        if let Some(offset) = dwarf
            .attr_ranges_offset(unit, ranges_attr)
            .map_err(|e| anyhow!("{}", e))?
        {
            let mut ranges = dwarf.ranges(unit, offset).map_err(|e| anyhow!("{}", e))?;
            while let Some(range) = ranges.next().map_err(|e| anyhow!("{}", e))? {
                size += range.end.saturating_sub(range.begin);
            }
        }
    }
    Ok(size)
}

fn count_nested_inlines<R: Reader>(
    unit: &Unit<R>,
    node: gimli::EntriesTreeNode<R>,
) -> Result<usize> {
    let mut count = 0;
    let mut children = node.children();
    while let Some(child_node) = children.next().map_err(|e| anyhow!("{}", e))? {
        let child = child_node.entry();
        if child.tag() == gimli::DW_TAG_inlined_subroutine {
            count += 1;
            count += count_nested_inlines(unit, child_node)?;
        } else {
            count += count_nested_inlines(unit, child_node)?;
        }
    }
    Ok(count)
}

fn collect_inlines_in_child<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    node: gimli::EntriesTreeNode<R>,
) -> Result<(u64, usize)> {
    let mut size = 0;
    let mut count = 0;
    let mut children = node.children();
    while let Some(child_node) = children.next().map_err(|e| anyhow!("{}", e))? {
        let child = child_node.entry();
        if child.tag() == gimli::DW_TAG_inlined_subroutine {
            size += get_entry_size(dwarf, unit, child)?;
            count += 1;
            count += count_nested_inlines(unit, child_node)?;
        } else {
            let (c_size, c_count) = collect_inlines_in_child(dwarf, unit, child_node)?;
            size += c_size;
            count += c_count;
        }
    }
    Ok((size, count))
}
