use crate::sys::{self, FsEntry};

const MAX_LINE: usize = 256;
const MAX_PATH: usize = 128;
const MAX_ENTRIES: usize = 64;

struct Shell {
    cwd: [u8; MAX_PATH],
    cwd_len: usize,
}

impl Shell {
    fn new() -> Self {
        let mut s = Self { cwd: [0; MAX_PATH], cwd_len: 1 };
        s.cwd[0] = b'/';
        s
    }

    fn cwd_str(&self) -> &str {
        core::str::from_utf8(&self.cwd[..self.cwd_len]).unwrap_or("/")
    }

    fn set_cwd(&mut self, path: &str) {
        let n = core::cmp::min(path.len(), MAX_PATH - 1);
        self.cwd[..n].copy_from_slice(&path.as_bytes()[..n]);
        self.cwd_len = n;
        if self.cwd_len == 0 { self.cwd[0] = b'/'; self.cwd_len = 1; }
    }

    fn resolve(&self, arg: &str, out: &mut [u8; MAX_PATH]) -> usize {
        let mut tmp = [0u8; MAX_PATH];
        let mut len = 0usize;
        if arg.starts_with('/') {
            tmp[0] = b'/'; len = 1;
        } else {
            let c = self.cwd_str().as_bytes();
            let n = core::cmp::min(c.len(), MAX_PATH - 1);
            tmp[..n].copy_from_slice(&c[..n]);
            len = n;
            if len == 0 { tmp[0]=b'/'; len=1; }
        }
        let start = if arg.starts_with('/') { 1 } else { 0 };
        for part in arg[start..].split('/') {
            if part.is_empty() || part == "." { continue; }
            if part == ".." {
                // go up
                if len > 1 {
                    if tmp[len-1] == b'/' { len -= 1; }
                    while len > 1 && tmp[len-1] != b'/' { len -= 1; }
                    if len > 1 && tmp[len-1] == b'/' { len -= 1; }
                    if len == 0 { tmp[0]=b'/'; len=1; }
                }
                continue;
            }
            if len > 1 && tmp[len-1] != b'/' { if len < MAX_PATH-1 { tmp[len]=b'/'; len+=1; } }
            let bytes = part.as_bytes();
            let copy = core::cmp::min(bytes.len(), MAX_PATH - 1 - len);
            tmp[len..len+copy].copy_from_slice(&bytes[..copy]);
            len += copy;
        }
        if len == 0 { tmp[0]=b'/'; len=1; }
        // trim trailing slash except root
        if len > 1 && tmp[len-1] == b'/' { len -= 1; }
        out[..len].copy_from_slice(&tmp[..len]);
        out[len] = 0;
        len
    }
}

fn path_str<'a>(buf: &'a [u8; MAX_PATH], len: usize) -> &'a str {
    core::str::from_utf8(&buf[..len]).unwrap_or("/")
}

fn print_help() {
    uprintln!("Commands:");
    uprintln!("  help                  - show this help");
    uprintln!("  ls [path]             - list directory");
    uprintln!("  cd <path>             - change directory");
    uprintln!("  pwd                   - print current directory");
    uprintln!("  cat <file>            - print file contents");
    uprintln!("  write <file> <text>   - create/overwrite file with text");
    uprintln!("  append <file> <text>  - append text to file");
    uprintln!("  mkdir <path>          - create directory");
    uprintln!("  touch <file>          - create empty file");
    uprintln!("  rm <path>             - remove file or empty directory");
    uprintln!("  cp <src> <dst>        - copy file");
    uprintln!("  mv <src> <dst>        - move/rename file");
    uprintln!("  stat <path>           - show file info");
    uprintln!("  echo <text>           - print text");
    uprintln!("  clear                 - clear screen (ANSI)");
    uprintln!("  exit                  - exit shell");
}

fn cmd_ls(sh: &Shell, arg: &str) {
    let mut pbuf = [0u8; MAX_PATH];
    let plen = if arg.is_empty() { sh.resolve(".", &mut pbuf) } else { sh.resolve(arg, &mut pbuf) };
    let path = path_str(&pbuf, plen);
    let mut entries = [FsEntry { name: [0;12], size: 0, is_dir: 0 }; MAX_ENTRIES];
    let Some(n) = sys::fs_list(path, &mut entries) else {
        uprintln!("ls: cannot access '{}'", path);
        return;
    };
    for i in 0..n {
        let e = &entries[i];
        let name_len = e.name.iter().position(|&c| c==0).unwrap_or(12);
        let name = core::str::from_utf8(&e.name[..name_len]).unwrap_or("?");
        if e.is_dir != 0 {
            uprintln!("{:<12}  <DIR>", name);
        } else {
            uprintln!("{:<12}  {} bytes", name, e.size);
        }
    }
    if n == 0 { uprintln!("(empty)"); }
}

fn cmd_cat(sh: &Shell, arg: &str) {
    if arg.is_empty() { uprintln!("usage: cat <file>"); return; }
    let mut pbuf = [0u8; MAX_PATH];
    let plen = sh.resolve(arg, &mut pbuf);
    let path = path_str(&pbuf, plen);
    let mut buf = [0u8; 4096];
    let Some(n) = sys::fs_read(path, &mut buf) else {
        uprintln!("cat: cannot read '{}'", path);
        return;
    };
    sys::write_raw(&buf[..n]);
    if n == 0 || buf[n-1] != b'\n' { sys::write_raw(b"\n"); }
}

fn cmd_write(sh: &Shell, arg: &str, append: bool) {
    // split first token (path) and rest (text)
    let mut it = arg.splitn(2, char::is_whitespace);
    let Some(file) = it.next() else { uprintln!("usage: write <file> <text>"); return; };
    let text = it.next().unwrap_or("").trim();
    let mut pbuf = [0u8; MAX_PATH];
    let plen = sh.resolve(file, &mut pbuf);
    let path = path_str(&pbuf, plen);
    let mut data = [0u8; 4096];
    let mut len = 0usize;
    if append {
        if let Some(n) = sys::fs_read(path, &mut data) { len = n; }
    }
    let tb = text.as_bytes();
    let copy = core::cmp::min(tb.len(), data.len().saturating_sub(len));
    data[len..len+copy].copy_from_slice(&tb[..copy]);
    len += copy;
    if len < data.len() { data[len] = b'\n'; len += 1; }
    if sys::fs_write(path, &data[..len]) { uprintln!("ok"); } else { uprintln!("write: failed '{}'", path); }
}

fn cmd_mkdir(sh: &Shell, arg: &str) {
    if arg.is_empty() { uprintln!("usage: mkdir <path>"); return; }
    let mut pbuf = [0u8; MAX_PATH];
    let plen = sh.resolve(arg, &mut pbuf);
    let path = path_str(&pbuf, plen);
    if sys::fs_mkdir(path) { uprintln!("ok"); } else { uprintln!("mkdir: failed '{}'", path); }
}

fn cmd_touch(sh: &Shell, arg: &str) {
    if arg.is_empty() { uprintln!("usage: touch <file>"); return; }
    let mut pbuf = [0u8; MAX_PATH];
    let plen = sh.resolve(arg, &mut pbuf);
    let path = path_str(&pbuf, plen);
    if sys::fs_stat(path).is_some() { uprintln!("ok"); return; }
    if sys::fs_write(path, &[]) { uprintln!("ok"); } else { uprintln!("touch: failed '{}'", path); }
}

fn cmd_rm(sh: &Shell, arg: &str) {
    if arg.is_empty() { uprintln!("usage: rm <path>"); return; }
    let mut pbuf = [0u8; MAX_PATH];
    let plen = sh.resolve(arg, &mut pbuf);
    let path = path_str(&pbuf, plen);
    if sys::fs_unlink(path) { uprintln!("ok"); } else { uprintln!("rm: failed '{}'", path); }
}

fn cmd_cp(sh: &Shell, arg: &str) {
    let mut it = arg.split_whitespace();
    let (Some(src), Some(dst)) = (it.next(), it.next()) else { uprintln!("usage: cp <src> <dst>"); return; };
    let mut sbuf=[0u8;MAX_PATH]; let slen=sh.resolve(src,&mut sbuf); let sp=path_str(&sbuf,slen);
    let mut dbuf=[0u8;MAX_PATH]; let dlen=sh.resolve(dst,&mut dbuf); let dp=path_str(&dbuf,dlen);
    let mut data=[0u8;4096];
    let Some(n)=sys::fs_read(sp,&mut data) else { uprintln!("cp: cannot read '{}'", sp); return; };
    if sys::fs_write(dp,&data[..n]) { uprintln!("ok"); } else { uprintln!("cp: cannot write '{}'", dp); }
}

fn cmd_mv(sh: &Shell, arg: &str) {
    let mut it = arg.split_whitespace();
    let (Some(src), Some(dst)) = (it.next(), it.next()) else { uprintln!("usage: mv <src> <dst>"); return; };
    let mut sbuf=[0u8;MAX_PATH]; let slen=sh.resolve(src,&mut sbuf); let sp=path_str(&sbuf,slen);
    let mut dbuf=[0u8;MAX_PATH]; let dlen=sh.resolve(dst,&mut dbuf); let dp=path_str(&dbuf,dlen);
    if sys::fs_rename(sp,dp) { uprintln!("ok"); } else { uprintln!("mv: failed"); }
}

fn cmd_stat(sh: &Shell, arg: &str) {
    if arg.is_empty() { uprintln!("usage: stat <path>"); return; }
    let mut pbuf=[0u8;MAX_PATH]; let plen=sh.resolve(arg,&mut pbuf); let path=path_str(&pbuf,plen);
    let Some(e)=sys::fs_stat(path) else { uprintln!("stat: not found '{}'", path); return; };
    let n=e.name.iter().position(|&c|c==0).unwrap_or(12);
    let name=core::str::from_utf8(&e.name[..n]).unwrap_or("?");
    uprintln!("name: {}", name);
    uprintln!("type: {}", if e.is_dir!=0 { "dir" } else { "file" });
    uprintln!("size: {}", e.size);
}

pub fn run() -> ! {
    uprintln!("rvsh: simple shell (type 'help')");
    let mut sh = Shell::new();
    let mut line = [0u8; MAX_LINE];
    loop {
        // prompt
        sys::write_raw(b"rvsh:");
        sys::write_raw(sh.cwd_str().as_bytes());
        sys::write_raw(b"$ ");
        let n = sys::read_line(&mut line);
        if n == 0 { continue; }
        let s = core::str::from_utf8(&line[..n]).unwrap_or("");
        let s = s.trim();
        if s.is_empty() { continue; }
        let mut parts = s.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        match cmd {
            "help" => print_help(),
            "ls" => cmd_ls(&sh, arg),
            "cd" => {
                let target = if arg.is_empty() { "/" } else { arg };
                let mut pbuf=[0u8;MAX_PATH]; let plen=sh.resolve(target,&mut pbuf); let p=path_str(&pbuf,plen);
                match sys::fs_stat(p) {
                    Some(e) if e.is_dir!=0 => sh.set_cwd(p),
                    Some(_) => uprintln!("cd: not a directory: {}", p),
                    None => uprintln!("cd: no such path: {}", p),
                }
            }
            "pwd" => uprintln!("{}", sh.cwd_str()),
            "cat" => cmd_cat(&sh, arg),
            "write" => cmd_write(&sh, arg, false),
            "append" => cmd_write(&sh, arg, true),
            "mkdir" => cmd_mkdir(&sh, arg),
            "touch" => cmd_touch(&sh, arg),
            "rm" => cmd_rm(&sh, arg),
            "cp" => cmd_cp(&sh, arg),
            "mv" => cmd_mv(&sh, arg),
            "stat" => cmd_stat(&sh, arg),
            "echo" => uprintln!("{}", arg),
            "clear" => sys::write_raw(b"\x1b[2J\x1b[H"),
            "exit" => sys::exit(0),
            _ => uprintln!("unknown command: {} (type 'help')", cmd),
        }
    }
}
