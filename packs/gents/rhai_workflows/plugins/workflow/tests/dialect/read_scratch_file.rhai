let meta = #{ name: "read-scratch-file", description: "read back a scratch file, and refuse one never written" };
write_scratch_file("notes.txt", "alpha\nbeta\n");
let lines = read_scratch_file("notes.txt").split("\n");
let missing = "found";
try { read_scratch_file("absent.txt"); } catch (e) { missing = e.split(":")[0]; }
complete(#{ lines: lines, missing: missing });
