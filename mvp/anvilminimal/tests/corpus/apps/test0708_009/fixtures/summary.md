Compile-repair recurrence (test0708_009):
- attempt 1 used an appended repair session and edited `src/app/page.tsx`, but
  the build still failed.
- attempt 2 used an appended repair session with `changed_paths=[]`; attempt 3 used compact with `changed_paths=[]`.
- historical fixture gap: single-file compile failure reached compact zero-edit without any `repair_regeneration` or regeneration skip decision event.
- run_stop false-success recording (historical): process exit success with release-gate failure.
