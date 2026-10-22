let meta = #{ name: "scratch-write-complete", description: "write a scratch file, then complete with its path" };
let path = write_scratch_file("report.md", "# Report\nall clear\n");
let again = write_scratch_file("report.md", "# Report\nall clear\n");
complete(#{ path: path, same_path: path == again, name: "report.md" });
