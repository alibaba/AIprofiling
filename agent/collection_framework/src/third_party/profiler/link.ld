SECTIONS
{
    .thread_func_sec :
    {
        __start_thread_func = .;
        KEEP(*(.thread_func_sec))
        __end_thread_func = .;
    }
}
INSERT AFTER .text;