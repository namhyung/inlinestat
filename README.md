# inlinestat

A simple command line utility to show statistics about function inlining
in an ELF binary file.  Started as my Rust programming practice. :)

    $ inlinestat --help
    Usage: inlinestat [OPTIONS] <FILE_NAME>
    
    Arguments:
      <FILE_NAME>
    
    Options:
      -l, --linkage-name
      -h, --help          Print help
      -V, --version       Print version

The output contains the following:
 * total size: size of the function in bytes.
 * self size: size of the function without inlined functions.
 * inlines: number of functions inlined (including nested ones).
 * function name: plain (leaf) name or linkage name.

```
$ inlinestat $(which uftrace) | grep -e ^# -e command_
# Total Sz    Self Sz  Inlines   Function Name
      2428       1006        5   command_dump
      3193        551       18   command_graph
       947        443        3   command_info
      2825        726       10   command_live
      1957        404        5   command_record
      4633        442       36   command_recv
      5318        506       30   command_replay
      3230        640       17   command_report
      1498        490        2   command_script
     10554        277      108   command_tui
```

