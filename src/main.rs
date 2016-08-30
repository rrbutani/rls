extern crate racer;
extern crate rand;

extern crate rustc_serialize;
use rustc_serialize::json;

use racer::core::complete_from_file;
use racer::core::find_definition;
use racer::core;
use racer::scopes;

use std::fs::{self, File};
use std::path::*;
use std::thread;

use std::net::{TcpListener, TcpStream};
use std::io::prelude::*;
use std::io;
use std::panic;

/// A temporary file that is removed on drop
///
/// With the new constructor, you provide contents and a file is created based on the name of the
/// current task. The with_name constructor allows you to choose a name. Neither forms are secure,
/// and both are subject to race conditions.
pub struct TmpFile {
    path_buf: PathBuf
}

impl TmpFile {
    /// Create a temp file with random name and `contents`.
    pub fn new(contents: &str) -> TmpFile {
        let tmp = TmpFile {
            path_buf: PathBuf::from(tmpname())
        };

        tmp.write_contents(contents);
        tmp
    }

    /// Create a file with `name` and `contents`.
    pub fn with_path<P: AsRef<Path>>(name: P, contents: &str) -> TmpFile {
        let tmp = TmpFile {
            path_buf: name.as_ref().to_path_buf()
        };

        tmp.write_contents(contents);
        tmp
    }

    /// Create a file with `name` and `contents`.
    pub fn with_name(name: &str, contents: &str) -> TmpFile {
        TmpFile::with_path(&Path::new(name), contents)
    }

    fn write_contents(&self, contents: &str) {
        let mut f = File::create(self.path()).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.flush().unwrap();
    }


    /// Get the Path of the TmpFile
    pub fn path<'a>(&'a self) -> &'a Path {
        self.path_buf.as_path()
    }
}

/// Make path for tmpfile
fn tmpname() -> String {
    use rand::Rng;

    let thread = thread::current();
    let taskname = thread.name().unwrap();
    let s = taskname.replace("::", "_");
    let mut p = "tmpfile.".to_string();
    p.push_str(&s[..]);
    // Add some random chars
    for c in ::rand::thread_rng().gen_ascii_chars().take(5) {
        p.push(c);
    }

    p
}

impl Drop for TmpFile {
    fn drop(&mut self) {
        fs::remove_file(self.path_buf.as_path()).unwrap();
    }
}

pub struct TmpDir {
    path_buf: PathBuf
}

impl TmpDir {
    pub fn new() -> TmpDir {
        TmpDir::with_name(&tmpname()[..])
    }

    pub fn with_name(name: &str) -> TmpDir {
        let pb = PathBuf::from(name);
        fs::create_dir_all(&pb).unwrap();

        TmpDir {
            path_buf: pb
        }
    }

    /// Create a new temp file in the directory.
    pub fn new_temp_file(&self, contents: &str) -> TmpFile {
        self.new_temp_file_with_name(&tmpname()[..], contents)
    }

    /// Create new temp file with name in the directory
    pub fn new_temp_file_with_name(&self, name: &str, contents: &str) -> TmpFile {
        let name = self.path_buf.join(name);
        TmpFile::with_path(name, contents)
    }

    pub fn pathbuf(&self) -> &PathBuf {
        &self.path_buf
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path_buf).unwrap();
    }
}

#[derive(RustcDecodable, RustcEncodable)]
struct Position {
    filepath: String,
    line: usize,
    col: usize
}

#[derive(Debug, RustcDecodable, RustcEncodable)]
struct Completion {
    name: String,
    context: String
}

#[derive(Debug, RustcDecodable, RustcEncodable)]
enum Command { GotoDef, Complete }

#[derive(Debug, RustcDecodable, RustcEncodable)]
struct Request {
    command: Command,
    filepath: String,
    line: usize,
    col: usize,
}

fn complete(source: Position) -> Vec<Completion> {
    let result = panic::catch_unwind (|| {
        let path = Path::new(&source.filepath);
        let mut f = File::open(&path).unwrap();
        let mut src = String::new();
        f.read_to_string(&mut src).unwrap();
        let pos = scopes::coords_to_point(&src, source.line, source.col);
        let cache = core::FileCache::new();
        let got = complete_from_file(&src, &path, pos, &core::Session::from_path(&cache, &path, &path));

        let mut results = vec![];
        for comp in got {
            results.push(Completion {name: comp.matchstr.clone(), context: comp.contextstr.clone() });
        }
        results
    });
    if let Ok(output) = result {
        output
    }
    else {
        vec![]
    }
}

fn goto_def(source: Position) -> Option<Position> {
    let result = panic::catch_unwind (|| {
        let path = Path::new(&source.filepath);
        let mut f = File::open(&path).unwrap();
        let mut src = String::new();
        f.read_to_string(&mut src).unwrap();
        let pos = scopes::coords_to_point(&src, source.line, source.col);
        let cache = core::FileCache::new();
        if let Some(mch) = find_definition(&src, &path,
                                pos,
                                &core::Session::from_path(&cache, &path, &path)) {
            let mut f = File::open(&mch.filepath).unwrap();
            let mut source_src = String::new();
            f.read_to_string(&mut source_src).unwrap();
            if mch.point != 0 {
                let (line, col) = scopes::point_to_coords(&source_src, mch.point);
                let fpath = mch.filepath.to_str().unwrap().to_string();
                Some(Position { filepath: fpath, line: line, col: col })
            }
            else {
                None
            }
        }
        else {
            None
        }
    });
    if let Ok(output) = result {
        output
    }
    else {
        None
    }
}

fn read_command(stream: &mut TcpStream) -> io::Result<Request> {
    let mut byte_buff: [u8; 1] = [0];
    let mut buffer = String::new();

    while let Ok(b) = stream.read(&mut byte_buff) {
        buffer.push(byte_buff[0] as char);
        if byte_buff[0] == b'}' {
            break;
        }
    }

    let res: Request = json::decode(&buffer).unwrap();

    Ok(res)
}

fn handle_client(mut stream: TcpStream) {
    while let Ok(request) = read_command(&mut stream) {
        let pos = Position { filepath: request.filepath, line: request.line, col: request.col};
        match request.command {
            Command::GotoDef => {
                if let Some(pos) = goto_def(pos) {
                    let reply = json::encode(&pos).unwrap();
                    stream.write(reply.as_bytes());
                }
                else {
                    println!("No match found");
                }
            }
            Command::Complete => {
                let completions = complete(pos);
                let reply = json::encode(&completions).unwrap();
                stream.write(reply.as_bytes());
            }
        }
    }
}

fn main() {
    use std::env;
    use std::borrow::Borrow;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:9000").unwrap();
    println!("Listening on 127.0.0.1:9000");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || {
                    handle_client(stream)
                });
            }
            Err(e) => {
                println!("Error with socket: {:?}", e);
            }
        }
    }
    drop(listener);
}