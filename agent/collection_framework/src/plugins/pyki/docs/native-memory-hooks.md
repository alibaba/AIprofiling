# How do native memory hooks work ?
- During startup, we save the original native memory APIs and then replace the corresponding GOT entries with our hooks.
- During shutdown, we restore the corresponding GOT entries to the original libc (or other) functions.
- Some libraries may not have been loaded via `dlopen()` at startup, so we also hook `dlopen()`. When a library is loaded later, we hook the native memory APIs in that library as well.
- Calling `malloc()` may cause the allocator to request new memory from the OS via `mmap()`. However, since both `malloc()` and `mmap()` are hooked, 
  recording both would overcount memory usage (i.e., the total would be larger than the actual amount).
  To avoid this, we add a recursion guard to each hook and record only the outermost hook (in this example, `malloc()` is recorded and `mmap()` is not).
- If we record every `malloc()` and `free()` and the invocation count is very large, the profile can also become very large.
  We follow an approach used by many open-source projects: for each allocation-size interval, we record only one sample. The observed total allocated size remains the same, 
  while the number of recorded `malloc()` calls is significantly reduced, preserving statistical correctness.
  In the future, we can implement a more sophisticated mechanism that maintains an in-memory map: on `malloc()`, insert an entry; on `free()`, remove it. 
  Then, only allocations that are not freed (i.e., leaks) are written to the profile file.