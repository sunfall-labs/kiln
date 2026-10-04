//! `node:fs` and `node:fs/promises` implementation.
//!
//! Provides filesystem operation bindings, options parsing, and property lookups.

use std::cell::RefCell;
use std::rc::Rc;

use crate::buffer::node_bytes_value;
use crate::{ArrayData, ExecutionError, HostOps, Native, ObjectData, RealmId, Value, Vm};

/// An open virtual file handle: either an in-memory buffer or an underlying OS file.
#[derive(Debug)]
pub(crate) enum VirtualFile {
    Memory {
        path: String,
        data: Vec<u8>,
        cursor: usize,
        readable: bool,
        writable: bool,
        #[allow(dead_code)]
        append: bool,
    },
    Os {
        #[allow(dead_code)]
        path: String,
        file: std::fs::File,
    },
}

/// A per-VM virtual file descriptor table.
///
/// Maps synthetic integer file descriptors to `VirtualFile` handles.
/// Virtual descriptors eliminate ambient raw OS FD leaks and collisions, allowing in-memory
/// VFS files to be opened, read, stat-inspected, and closed with synthetic FDs.
#[derive(Debug)]
pub(crate) struct VirtualFdTable {
    entries: std::collections::BTreeMap<u32, VirtualFile>,
    next_fd: u32,
}

impl Default for VirtualFdTable {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualFdTable {
    pub(crate) fn new() -> Self {
        Self {
            entries: std::collections::BTreeMap::new(),
            next_fd: 10,
        }
    }

    pub(crate) fn insert(&mut self, file: VirtualFile) -> u32 {
        let fd = self.next_fd;
        self.next_fd = self.next_fd.checked_add(1).unwrap_or(10);
        self.entries.insert(fd, file);
        fd
    }

    pub(crate) fn get(&self, fd: u32) -> Option<&VirtualFile> {
        self.entries.get(&fd)
    }

    pub(crate) fn get_mut(&mut self, fd: u32) -> Option<&mut VirtualFile> {
        self.entries.get_mut(&fd)
    }

    pub(crate) fn remove(&mut self, fd: u32) -> Option<VirtualFile> {
        self.entries.remove(&fd)
    }
}

/// Whether a Node read option requests a UTF-8 string rather than a Buffer-like byte view.
pub(crate) fn node_encoding(option: Option<&Value>) -> Result<bool, ExecutionError> {
    match option {
        None | Some(Value::Undefined) | Some(Value::Null) => Ok(false),
        Some(Value::String(encoding)) => {
            parse_node_encoding(&encoding.to_scalar_string().map_err(|_| {
                ExecutionError::TypeErrorMessage {
                    message: "encoding contains a lone surrogate".to_owned(),
                }
            })?)
        }
        Some(Value::Object(options)) => match options.borrow().get_own("encoding") {
            None | Some(Value::Undefined) | Some(Value::Null) => Ok(false),
            Some(Value::String(encoding)) => {
                parse_node_encoding(&encoding.to_scalar_string().map_err(|_| {
                    ExecutionError::TypeErrorMessage {
                        message: "encoding contains a lone surrogate".to_owned(),
                    }
                })?)
            }
            Some(value) => Err(ExecutionError::TypeErrorMessage {
                message: format!("encoding must be a string, received {}", value.type_name()),
            }),
        },
        Some(value) => Err(ExecutionError::TypeErrorMessage {
            message: format!(
                "readFile options must be a string or object, received {}",
                value.type_name()
            ),
        }),
    }
}

pub(crate) fn parse_node_encoding(encoding: &str) -> Result<bool, ExecutionError> {
    match encoding.to_ascii_lowercase().as_str() {
        "utf8" | "utf-8" => Ok(true),
        other => Err(ExecutionError::TypeErrorMessage {
            message: format!("unsupported file encoding {other:?}"),
        }),
    }
}

pub(crate) fn constants_object() -> Value {
    let mut obj = ObjectData::default();
    obj.set("F_OK", Value::Number(0.0));
    obj.set("R_OK", Value::Number(4.0));
    obj.set("W_OK", Value::Number(2.0));
    obj.set("X_OK", Value::Number(1.0));
    obj.set("COPYFILE_EXCL", Value::Number(1.0));
    obj.set("COPYFILE_FICLONE", Value::Number(2.0));
    obj.set("COPYFILE_FICLONE_FORCE", Value::Number(4.0));
    Value::Object(Rc::new(RefCell::new(obj)))
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "existsSync" => Some(Value::Native(Native::NodeFsExistsSync, RealmId::PRIMARY)),
        "readFileSync" => Some(Value::Native(Native::NodeFsReadFileSync, RealmId::PRIMARY)),
        "mkdirSync" => Some(Value::Native(Native::NodeFsMkdirSync, RealmId::PRIMARY)),
        "writeFileSync" => Some(Value::Native(Native::NodeFsWriteFileSync, RealmId::PRIMARY)),
        "statSync" => Some(Value::Native(Native::NodeFsStatSync, RealmId::PRIMARY)),
        "lstatSync" => Some(Value::Native(Native::NodeFsLstatSync, RealmId::PRIMARY)),
        "fstatSync" => Some(Value::Native(Native::NodeFsFstatSync, RealmId::PRIMARY)),
        "readdirSync" => Some(Value::Native(Native::NodeFsReaddirSync, RealmId::PRIMARY)),
        "rmSync" => Some(Value::Native(Native::NodeFsRmSync, RealmId::PRIMARY)),
        "unlinkSync" => Some(Value::Native(Native::NodeFsUnlinkSync, RealmId::PRIMARY)),
        "realpathSync" => Some(Value::Native(Native::NodeFsRealpathSync, RealmId::PRIMARY)),
        "renameSync" => Some(Value::Native(Native::NodeFsRenameSync, RealmId::PRIMARY)),
        "copyFileSync" => Some(Value::Native(Native::NodeFsCopyFileSync, RealmId::PRIMARY)),
        "accessSync" => Some(Value::Native(Native::NodeFsAccessSync, RealmId::PRIMARY)),
        "appendFileSync" => Some(Value::Native(
            Native::NodeFsAppendFileSync,
            RealmId::PRIMARY,
        )),
        "chmodSync" => Some(Value::Native(Native::NodeFsChmodSync, RealmId::PRIMARY)),
        "fchmodSync" => Some(Value::Native(Native::NodeFsFchmodSync, RealmId::PRIMARY)),
        "openSync" => Some(Value::Native(Native::NodeFsOpenSync, RealmId::PRIMARY)),
        "closeSync" => Some(Value::Native(Native::NodeFsCloseSync, RealmId::PRIMARY)),
        "readlinkSync" => Some(Value::Native(Native::NodeFsReadlinkSync, RealmId::PRIMARY)),
        "rmdirSync" => Some(Value::Native(Native::NodeFsRmdirSync, RealmId::PRIMARY)),
        "utimesSync" => Some(Value::Native(Native::NodeFsUtimesSync, RealmId::PRIMARY)),
        "futimesSync" => Some(Value::Native(Native::NodeFsFutimesSync, RealmId::PRIMARY)),
        "readSync" => Some(Value::Native(Native::NodeFsSync, RealmId::PRIMARY)),
        "writeSync" => Some(Value::Native(Native::NodeFsWriteSync, RealmId::PRIMARY)),
        "ftruncateSync" | "truncateSync" => {
            Some(Value::Native(Native::NodeFsFtruncateSync, RealmId::PRIMARY))
        }
        "fsyncSync" | "fdatasyncSync" => {
            Some(Value::Native(Native::NodeFsFsyncSync, RealmId::PRIMARY))
        }
        "constants" => Some(constants_object()),
        "promises" => Some(Value::Native(Native::NodeFsPromises, RealmId::PRIMARY)),
        "default" => Some(Value::Native(Native::NodeFs, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) fn promises_property(prop: &str) -> Option<Value> {
    match prop {
        "appendFile" => Some(Value::Native(Native::NodeFsAppendFile, RealmId::PRIMARY)),
        "readFile" => Some(Value::Native(Native::NodeFsReadFile, RealmId::PRIMARY)),
        "writeFile" => Some(Value::Native(Native::NodeFsWriteFile, RealmId::PRIMARY)),
        "stat" => Some(Value::Native(Native::NodeFsStat, RealmId::PRIMARY)),
        "mkdir" => Some(Value::Native(Native::NodeFsMkdir, RealmId::PRIMARY)),
        "readdir" => Some(Value::Native(Native::NodeFsReaddir, RealmId::PRIMARY)),
        "copyFile" => Some(Value::Native(Native::NodeFsCopyFile, RealmId::PRIMARY)),
        "rename" => Some(Value::Native(Native::NodeFsRename, RealmId::PRIMARY)),
        "rm" => Some(Value::Native(Native::NodeFsRm, RealmId::PRIMARY)),
        "unlink" => Some(Value::Native(Native::NodeFsUnlink, RealmId::PRIMARY)),
        "lstat" => Some(Value::Native(Native::NodeFsLstat, RealmId::PRIMARY)),
        "realpath" => Some(Value::Native(Native::NodeFsRealpath, RealmId::PRIMARY)),
        "access" => Some(Value::Native(Native::NodeFsAccess, RealmId::PRIMARY)),
        "chmod" => Some(Value::Native(Native::NodeFsChmod, RealmId::PRIMARY)),
        "rmdir" => Some(Value::Native(Native::NodeFsRmdir, RealmId::PRIMARY)),
        "readlink" => Some(Value::Native(Native::NodeFsReadlink, RealmId::PRIMARY)),
        "constants" => Some(constants_object()),
        "default" => Some(Value::Native(Native::NodeFsPromises, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeFs => Some("fs"),
        Native::NodeFsPromises => Some("promises"),
        Native::NodeFsExistsSync => Some("existsSync"),
        Native::NodeFsReadFileSync => Some("readFileSync"),
        Native::NodeFsMkdirSync => Some("mkdirSync"),
        Native::NodeFsWriteFileSync => Some("writeFileSync"),
        Native::NodeFsStatSync => Some("statSync"),
        Native::NodeFsLstatSync => Some("lstatSync"),
        Native::NodeFsFstatSync => Some("fstatSync"),
        Native::NodeFsReaddirSync => Some("readdirSync"),
        Native::NodeFsRmSync => Some("rmSync"),
        Native::NodeFsUnlinkSync => Some("unlinkSync"),
        Native::NodeFsRealpathSync => Some("realpathSync"),
        Native::NodeFsRenameSync => Some("renameSync"),
        Native::NodeFsCopyFileSync => Some("copyFileSync"),
        Native::NodeFsAccessSync => Some("accessSync"),
        Native::NodeFsAppendFileSync => Some("appendFileSync"),
        Native::NodeFsChmodSync => Some("chmodSync"),
        Native::NodeFsFchmodSync => Some("fchmodSync"),
        Native::NodeFsOpenSync => Some("openSync"),
        Native::NodeFsCloseSync => Some("closeSync"),
        Native::NodeFsReadlinkSync => Some("readlinkSync"),
        Native::NodeFsRmdirSync => Some("rmdirSync"),
        Native::NodeFsUtimesSync => Some("utimesSync"),
        Native::NodeFsFutimesSync => Some("futimesSync"),
        Native::NodeFsReadFile => Some("readFile"),
        Native::NodeFsWriteFile => Some("writeFile"),
        Native::NodeFsAppendFile => Some("appendFile"),
        Native::NodeFsStat => Some("stat"),
        Native::NodeFsMkdir => Some("mkdir"),
        Native::NodeFsReaddir => Some("readdir"),
        Native::NodeFsCopyFile => Some("copyFile"),
        Native::NodeFsRename => Some("rename"),
        Native::NodeFsRm => Some("rm"),
        Native::NodeFsUnlink => Some("unlink"),
        Native::NodeFsLstat => Some("lstat"),
        Native::NodeFsRealpath => Some("realpath"),
        Native::NodeFsAccess => Some("access"),
        Native::NodeFsChmod => Some("chmod"),
        Native::NodeFsRmdir => Some("rmdir"),
        Native::NodeFsReadlink => Some("readlink"),
        Native::NodeStatIsFile => Some("isFile"),
        Native::NodeStatIsDirectory => Some("isDirectory"),
        Native::NodeStatIsSymbolicLink => Some("isSymbolicLink"),
        Native::NodeFsSync => Some("readSync"),
        Native::NodeFsWriteSync => Some("writeSync"),
        Native::NodeFsFtruncateSync => Some("ftruncateSync"),
        Native::NodeFsFsyncSync => Some("fsyncSync"),
        _ => None,
    }
}

fn throw_node_error<H: HostOps + ?Sized>(
    vm: &mut Vm<'_, H>,
    syscall: &str,
    path: Option<String>,
    err: std::io::Error,
) -> ExecutionError {
    let code = match err.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::AlreadyExists => "EEXIST",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        _ => "EIO",
    };
    let message = if let Some(ref p) = path {
        format!("{code}: {err}, {syscall} '{p}'")
    } else {
        format!("{code}: {err}, {syscall}")
    };
    let error = kiln_host_abi::AsyncHostError {
        os_errno: err.raw_os_error(),
        code: code.to_owned(),
        message,
        syscall: syscall.to_owned(),
        path,
    };
    let err_val = vm.node_error_value(error);
    vm.caught = Some(err_val);
    ExecutionError::ThrownAcrossNative
}

fn with_fs<H: HostOps + ?Sized, R>(
    vm: &mut Vm<'_, H>,
    f: impl FnOnce(&mut dyn kiln_host_abi::FileSystemOps) -> R,
) -> R {
    if let Some(fs) = vm.host.fs() {
        f(fs)
    } else {
        f(&mut kiln_host_abi::OsFileSystem)
    }
}

pub(crate) fn stats_object(meta: &kiln_host_abi::FsMetadata) -> Value {
    let mut object = ObjectData::default();
    let is_file = meta.is_file;
    let is_dir = meta.is_dir;
    let is_symlink = meta.is_symlink;
    let size = meta.len as f64;
    let mtime_ms = meta.modified_ms as f64;
    let birthtime_ms = meta.created_ms as f64;
    let mode = meta.mode as f64;

    object.set("size", Value::Number(size));
    object.set("mtimeMs", Value::Number(mtime_ms));
    object.set("ctimeMs", Value::Number(mtime_ms));
    object.set("atimeMs", Value::Number(mtime_ms));
    object.set("birthtimeMs", Value::Number(birthtime_ms));
    object.set("mtime", crate::new_date(mtime_ms));
    object.set("ctime", crate::new_date(mtime_ms));
    object.set("atime", crate::new_date(mtime_ms));
    object.set("birthtime", crate::new_date(birthtime_ms));
    object.set("mode", Value::Number(mode));
    object.set_non_enumerable("@@kiln:node:is-file", Value::Boolean(is_file));
    object.set_non_enumerable("@@kiln:node:is-directory", Value::Boolean(is_dir));
    object.set_non_enumerable("@@kiln:node:is-symlink", Value::Boolean(is_symlink));
    object.set(
        "isFile",
        Value::Native(Native::NodeStatIsFile, RealmId::PRIMARY),
    );
    object.set(
        "isDirectory",
        Value::Native(Native::NodeStatIsDirectory, RealmId::PRIMARY),
    );
    object.set(
        "isSymbolicLink",
        Value::Native(Native::NodeStatIsSymbolicLink, RealmId::PRIMARY),
    );
    Value::Object(Rc::new(RefCell::new(object)))
}

pub(crate) fn dispatch<H: HostOps + ?Sized>(
    vm: &mut Vm<'_, H>,
    native: Native,
    _receiver: &Value,
    arguments: &[Value],
    _instruction: usize,
) -> Result<Value, ExecutionError> {
    match native {
        Native::NodeFsExistsSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let exists = !path_str.is_empty()
                && with_fs(vm, |fs| fs.exists(std::path::Path::new(&path_str)));
            Ok(Value::Boolean(exists))
        }
        Native::NodeFsReadFileSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let read_res = with_fs(vm, |fs| fs.read(std::path::Path::new(&path_str)));
            match read_res {
                Ok(bytes) => {
                    let is_utf8 = arguments.get(1).map_or(false, |opt| match opt {
                        Value::String(s) => {
                            let lossy = s.to_lossy_string();
                            lossy == "utf8" || lossy == "utf-8"
                        }
                        Value::Object(obj) => {
                            obj.borrow().get("encoding").map_or(false, |val| match val {
                                Value::String(s) => {
                                    let lossy = s.to_lossy_string();
                                    lossy == "utf8" || lossy == "utf-8"
                                }
                                _ => false,
                            })
                        }
                        _ => false,
                    });
                    if is_utf8 {
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        Ok(Value::String(text.into()))
                    } else {
                        Ok(node_bytes_value(bytes))
                    }
                }
                Err(err) => Err(throw_node_error(vm, "open", Some(path_str), err)),
            }
        }
        Native::NodeFsWriteFileSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let data = match arguments.get(1) {
                Some(Value::String(s)) => s.to_lossy_string().into_bytes(),
                Some(Value::TypedArray(view)) => view.bytes(),
                Some(value) => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "data must be a string or typed array, received {}",
                            value.type_name()
                        ),
                    });
                }
                None => Vec::new(),
            };
            let mode = match arguments.get(2) {
                Some(Value::Number(n)) => Some(*n as u32),
                Some(Value::Object(obj)) => obj.borrow().get_own("mode").and_then(|v| match v {
                    Value::Number(n) => Some(*n as u32),
                    _ => None,
                }),
                _ => None,
            };
            let write_res = with_fs(vm, |fs| {
                let path = std::path::Path::new(&path_str);
                fs.write(path, &data)?;
                if let Some(mode) = mode {
                    fs.set_permissions(path, mode)?;
                }
                Ok::<(), std::io::Error>(())
            });
            if let Err(err) = write_res {
                return Err(throw_node_error(vm, "open", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsAppendFileSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let data = match arguments.get(1) {
                Some(Value::String(s)) => s.to_lossy_string().into_bytes(),
                Some(Value::TypedArray(view)) => view.bytes(),
                Some(value) => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "data must be a string or typed array, received {}",
                            value.type_name()
                        ),
                    });
                }
                None => Vec::new(),
            };
            let append_res = with_fs(vm, |fs| fs.append(std::path::Path::new(&path_str), &data));
            if let Err(err) = append_res {
                return Err(throw_node_error(vm, "write", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsMkdirSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let recursive = match arguments.get(1) {
                Some(Value::Boolean(b)) => *b,
                Some(Value::Object(obj)) => obj
                    .borrow()
                    .get_own("recursive")
                    .map_or(false, Value::is_truthy),
                _ => false,
            };
            let res = with_fs(vm, |fs| {
                let path = std::path::Path::new(&path_str);
                if recursive {
                    fs.create_dir_all(path)
                } else {
                    fs.create_dir(path)
                }
            });
            if let Err(err) = res {
                return Err(throw_node_error(vm, "mkdir", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsStatSync | Native::NodeFsLstatSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let throw_if_no_entry = match arguments.get(1) {
                Some(Value::Object(obj)) => obj
                    .borrow()
                    .get_own("throwIfNoEntry")
                    .map_or(true, Value::is_truthy),
                _ => true,
            };
            let is_lstat = matches!(native, Native::NodeFsLstatSync);
            let syscall = if is_lstat { "lstat" } else { "stat" };
            let meta_res = with_fs(vm, |fs| {
                let path = std::path::Path::new(&path_str);
                if is_lstat {
                    fs.symlink_metadata(path)
                } else {
                    fs.metadata(path)
                }
            });
            let meta = match meta_res {
                Ok(m) => m,
                Err(err) => {
                    if !throw_if_no_entry && err.kind() == std::io::ErrorKind::NotFound {
                        return Ok(Value::Undefined);
                    }
                    return Err(throw_node_error(vm, syscall, Some(path_str), err));
                }
            };
            Ok(stats_object(&meta))
        }
        Native::NodeFsFstatSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as u32;
            if let Some(vfile) = vm.fd_table.get(fd) {
                match vfile {
                    VirtualFile::Memory { data, .. } => {
                        let fsmeta = kiln_host_abi::FsMetadata::file(data.len() as u64, 0);
                        Ok(stats_object(&fsmeta))
                    }
                    VirtualFile::Os { file, .. } => match file.metadata() {
                        Ok(meta) => {
                            let fsmeta = kiln_host_abi::FsMetadata::from(&meta);
                            Ok(stats_object(&fsmeta))
                        }
                        Err(err) => Err(throw_node_error(vm, "fstat", None, err)),
                    },
                }
            } else {
                let err = std::io::Error::from_raw_os_error(9);
                Err(throw_node_error(vm, "fstat", None, err))
            }
        }
        Native::NodeFsReaddirSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let with_file_types = match arguments.get(1) {
                Some(Value::Object(obj)) => obj
                    .borrow()
                    .get_own("withFileTypes")
                    .map_or(false, Value::is_truthy),
                _ => false,
            };
            let read_dir_res = with_fs(vm, |fs| fs.read_dir(std::path::Path::new(&path_str)));
            let entries = match read_dir_res {
                Ok(rd) => rd,
                Err(err) => return Err(throw_node_error(vm, "scandir", Some(path_str), err)),
            };
            let mut out = Vec::new();
            for entry in entries {
                let name = entry.name;
                if with_file_types {
                    let mut dirent = ObjectData::default();
                    dirent.set("name", Value::String(name.into()));
                    dirent.set_non_enumerable("@@kiln:node:is-file", Value::Boolean(entry.is_file));
                    dirent.set_non_enumerable(
                        "@@kiln:node:is-directory",
                        Value::Boolean(entry.is_directory),
                    );
                    dirent.set_non_enumerable(
                        "@@kiln:node:is-symlink",
                        Value::Boolean(entry.is_symlink),
                    );
                    dirent.set(
                        "isFile",
                        Value::Native(Native::NodeStatIsFile, RealmId::PRIMARY),
                    );
                    dirent.set(
                        "isDirectory",
                        Value::Native(Native::NodeStatIsDirectory, RealmId::PRIMARY),
                    );
                    dirent.set(
                        "isSymbolicLink",
                        Value::Native(Native::NodeStatIsSymbolicLink, RealmId::PRIMARY),
                    );
                    out.push(Value::Object(Rc::new(RefCell::new(dirent))));
                } else {
                    out.push(Value::String(name.into()));
                }
            }
            Ok(Value::Array(ArrayData::new(out)))
        }
        Native::NodeFsRmSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let (recursive, force) = match arguments.get(1) {
                Some(Value::Object(obj)) => {
                    let rec = obj
                        .borrow()
                        .get_own("recursive")
                        .map_or(false, Value::is_truthy);
                    let frc = obj
                        .borrow()
                        .get_own("force")
                        .map_or(false, Value::is_truthy);
                    (rec, frc)
                }
                _ => (false, false),
            };
            let path = std::path::Path::new(&path_str);
            let exists = with_fs(vm, |fs| fs.exists(path));
            if !exists {
                if force {
                    return Ok(Value::Undefined);
                }
                let err =
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
                return Err(throw_node_error(vm, "rm", Some(path_str), err));
            }
            let res = with_fs(vm, |fs| {
                let is_dir = fs.metadata(path).map(|m| m.is_dir).unwrap_or(false);
                if is_dir {
                    if recursive {
                        fs.remove_dir_all(path)
                    } else {
                        fs.remove_dir(path)
                    }
                } else {
                    fs.remove_file(path)
                }
            });
            if let Err(err) = res {
                if force && err.kind() == std::io::ErrorKind::NotFound {
                    return Ok(Value::Undefined);
                }
                return Err(throw_node_error(vm, "rm", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsUnlinkSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let res = with_fs(vm, |fs| fs.remove_file(std::path::Path::new(&path_str)));
            if let Err(err) = res {
                return Err(throw_node_error(vm, "unlink", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsRealpathSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let res = with_fs(vm, |fs| fs.canonicalize(std::path::Path::new(&path_str)));
            match res {
                Ok(canon) => Ok(Value::String(canon.to_string_lossy().into_owned().into())),
                Err(err) => Err(throw_node_error(vm, "realpath", Some(path_str), err)),
            }
        }
        Native::NodeFsRenameSync => {
            let old_path = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let new_path = arguments
                .get(1)
                .map_or_else(String::new, Value::to_display_string);
            let res = with_fs(vm, |fs| {
                fs.rename(
                    std::path::Path::new(&old_path),
                    std::path::Path::new(&new_path),
                )
            });
            if let Err(err) = res {
                return Err(throw_node_error(vm, "rename", Some(old_path), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsCopyFileSync => {
            let src = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let dest = arguments
                .get(1)
                .map_or_else(String::new, Value::to_display_string);
            let res = with_fs(vm, |fs| {
                fs.copy(std::path::Path::new(&src), std::path::Path::new(&dest))
            });
            if let Err(err) = res {
                return Err(throw_node_error(vm, "copyfile", Some(src), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsAccessSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let exists = with_fs(vm, |fs| fs.exists(std::path::Path::new(&path_str)));
            if !exists {
                let err =
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
                return Err(throw_node_error(vm, "access", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsChmodSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let mode = arguments.get(1).map_or(0.0, Value::as_number) as u32;
            let res = with_fs(vm, |fs| {
                fs.set_permissions(std::path::Path::new(&path_str), mode)
            });
            if let Err(err) = res {
                return Err(throw_node_error(vm, "chmod", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsFchmodSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as u32;
            let mode = arguments.get(1).map_or(0.0, Value::as_number) as u32;
            let vfile = match vm.fd_table.get_mut(fd) {
                Some(f) => f,
                None => {
                    let err = std::io::Error::from_raw_os_error(9);
                    return Err(throw_node_error(vm, "fchmod", None, err));
                }
            };
            let mut err_res = None;
            match vfile {
                VirtualFile::Memory { .. } => {}
                VirtualFile::Os { file, .. } => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let permissions = std::fs::Permissions::from_mode(mode);
                        if let Err(err) = file.set_permissions(permissions) {
                            err_res = Some(err);
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = mode;
                    }
                }
            }
            if let Some(err) = err_res {
                return Err(throw_node_error(vm, "fchmod", None, err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsOpenSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let flags = arguments
                .get(1)
                .map_or_else(|| "r".to_string(), Value::to_display_string);

            let (readable, writable, append, create, truncate, create_new) = match flags.as_str() {
                "r" | "rs" => (true, false, false, false, false, false),
                "r+" | "rs+" => (true, true, false, false, false, false),
                "w" => (false, true, false, true, true, false),
                "wx" | "xw" => (false, true, false, true, false, true),
                "w+" => (true, true, false, true, true, false),
                "wx+" | "xw+" => (true, true, false, true, false, true),
                "a" => (false, true, true, true, false, false),
                "ax" | "xa" => (false, true, true, true, false, true),
                "a+" => (true, true, true, true, false, false),
                "ax+" | "xa+" => (true, true, true, true, false, true),
                _ => (true, false, false, false, false, false),
            };

            let is_in_mem = with_fs(vm, |fs| fs.is_in_memory(std::path::Path::new(&path_str)));
            if is_in_mem {
                if create_new {
                    return Err(throw_node_error(
                        vm,
                        "open",
                        Some(path_str),
                        std::io::Error::from_raw_os_error(17),
                    ));
                }
                let data = if truncate {
                    let _ = with_fs(vm, |fs| fs.write(std::path::Path::new(&path_str), &[]));
                    Vec::new()
                } else {
                    with_fs(vm, |fs| fs.read(std::path::Path::new(&path_str))).unwrap_or_default()
                };
                let cursor = if append { data.len() } else { 0 };
                let fd = vm.fd_table.insert(VirtualFile::Memory {
                    path: path_str,
                    data,
                    cursor,
                    readable,
                    writable,
                    append,
                });
                return Ok(Value::Number(fd as f64));
            }

            let mut opts = std::fs::OpenOptions::new();
            opts.read(readable);
            if writable {
                opts.write(true);
            }
            if append {
                opts.append(true);
            }
            if truncate {
                opts.truncate(true);
            }
            if create_new {
                opts.create_new(true);
            } else if create {
                opts.create(true);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                if let Some(Value::Number(mode)) = arguments.get(2) {
                    opts.mode(*mode as u32);
                }
            }
            let file = match opts.open(&path_str) {
                Ok(f) => f,
                Err(err) => return Err(throw_node_error(vm, "open", Some(path_str), err)),
            };
            let fd = vm.fd_table.insert(VirtualFile::Os {
                path: path_str,
                file,
            });
            Ok(Value::Number(fd as f64))
        }
        Native::NodeFsCloseSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as u32;
            if let Some(vfile) = vm.fd_table.remove(fd) {
                match vfile {
                    VirtualFile::Memory {
                        path,
                        data,
                        writable,
                        ..
                    } => {
                        if writable {
                            let _ = with_fs(vm, |fs| fs.write(std::path::Path::new(&path), &data));
                        }
                    }
                    VirtualFile::Os { file, .. } => {
                        drop(file);
                    }
                }
                Ok(Value::Undefined)
            } else {
                let err = std::io::Error::from_raw_os_error(9);
                Err(throw_node_error(vm, "close", None, err))
            }
        }
        Native::NodeFsReadlinkSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let res = with_fs(vm, |fs| fs.read_link(std::path::Path::new(&path_str)));
            match res {
                Ok(target) => Ok(Value::String(target.to_string_lossy().into_owned().into())),
                Err(err) => Err(throw_node_error(vm, "readlink", Some(path_str), err)),
            }
        }
        Native::NodeFsRmdirSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let mut recursive = false;
            if let Some(Value::Object(opts)) = arguments.get(1) {
                if let Some(Value::Boolean(rec)) = opts.borrow().get("recursive") {
                    recursive = rec;
                }
            }
            let res = with_fs(vm, |fs| {
                let path = std::path::Path::new(&path_str);
                if recursive {
                    fs.remove_dir_all(path)
                } else {
                    fs.remove_dir(path)
                }
            });
            if let Err(err) = res {
                return Err(throw_node_error(vm, "rmdir", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsUtimesSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let parse_time = |val: Option<&Value>| -> Option<std::time::SystemTime> {
                let ms = match val? {
                    Value::Date(d) => d.time.get(),
                    Value::Number(n) => n * 1000.0,
                    Value::String(s) => s.to_lossy_string().parse::<f64>().ok()? * 1000.0,
                    _ => return None,
                };
                if ms.is_nan() || ms.is_infinite() {
                    return None;
                }
                if ms >= 0.0 {
                    Some(
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_millis(ms as u64),
                    )
                } else {
                    Some(
                        std::time::SystemTime::UNIX_EPOCH
                            - std::time::Duration::from_millis((-ms) as u64),
                    )
                }
            };
            let atime = parse_time(arguments.get(1));
            let mtime = parse_time(arguments.get(2));
            if let (Some(atime), Some(mtime)) = (atime, mtime) {
                let mut times = std::fs::FileTimes::new();
                times = times.set_accessed(atime);
                times = times.set_modified(mtime);
                let res = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path_str)
                    .or_else(|_| std::fs::File::open(&path_str))
                    .and_then(|file| file.set_times(times));
                if let Err(err) = res {
                    return Err(throw_node_error(vm, "utime", Some(path_str), err));
                }
            } else {
                let err =
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid time value");
                return Err(throw_node_error(vm, "utime", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsFutimesSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as u32;
            let parse_time = |val: Option<&Value>| -> Option<std::time::SystemTime> {
                let ms = match val? {
                    Value::Date(d) => d.time.get(),
                    Value::Number(n) => n * 1000.0,
                    Value::String(s) => s.to_lossy_string().parse::<f64>().ok()? * 1000.0,
                    _ => return None,
                };
                if ms.is_nan() || ms.is_infinite() {
                    return None;
                }
                if ms >= 0.0 {
                    Some(
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_millis(ms as u64),
                    )
                } else {
                    Some(
                        std::time::SystemTime::UNIX_EPOCH
                            - std::time::Duration::from_millis((-ms) as u64),
                    )
                }
            };
            let atime = parse_time(arguments.get(1));
            let mtime = parse_time(arguments.get(2));
            let (atime, mtime) = match (atime, mtime) {
                (Some(a), Some(m)) => (a, m),
                _ => {
                    let err =
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid time value");
                    return Err(throw_node_error(vm, "futimes", None, err));
                }
            };

            let vfile = match vm.fd_table.get_mut(fd) {
                Some(f) => f,
                None => {
                    let err = std::io::Error::from_raw_os_error(9);
                    return Err(throw_node_error(vm, "futimes", None, err));
                }
            };

            let mut err_res = None;
            match vfile {
                VirtualFile::Memory { .. } => {}
                VirtualFile::Os { file, .. } => {
                    let mut times = std::fs::FileTimes::new();
                    times = times.set_accessed(atime);
                    times = times.set_modified(mtime);
                    if let Err(err) = file.set_times(times) {
                        err_res = Some(err);
                    }
                }
            }
            if let Some(err) = err_res {
                return Err(throw_node_error(vm, "futimes", None, err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as u32;
            let view = match arguments.get(1) {
                Some(Value::TypedArray(v)) => Rc::clone(v),
                other => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "The \"buffer\" argument must be an instance of Buffer or TypedArray, received {}",
                            other.map_or("undefined", Value::type_name)
                        ),
                    });
                }
            };
            let view_len = view.bytes().len();
            let offset = arguments.get(2).map_or(0.0, Value::as_number).max(0.0) as usize;
            let default_len = view_len.saturating_sub(offset);
            let length = arguments
                .get(3)
                .map_or(default_len as f64, Value::as_number)
                .max(0.0) as usize;
            let position = match arguments.get(4) {
                Some(Value::Number(n)) if *n >= 0.0 => Some(*n as u64),
                _ => None,
            };

            if offset > view_len {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "offset is out of bounds".to_owned(),
                });
            }

            let max_read = (view_len - offset).min(length);

            let vfile = match vm.fd_table.get_mut(fd) {
                Some(f) => f,
                None => {
                    let err = std::io::Error::from_raw_os_error(9);
                    return Err(throw_node_error(vm, "read", None, err));
                }
            };

            let bytes_read = match vfile {
                VirtualFile::Memory {
                    data,
                    cursor,
                    readable,
                    ..
                } => {
                    if !*readable {
                        let err = std::io::Error::from_raw_os_error(9);
                        return Err(throw_node_error(vm, "read", None, err));
                    }
                    let read_pos = position.unwrap_or(*cursor as u64) as usize;
                    if read_pos >= data.len() {
                        0
                    } else {
                        let available = &data[read_pos..];
                        let to_copy = max_read.min(available.len());
                        let written = view.write_bytes(offset, &available[..to_copy]);
                        if position.is_none() {
                            *cursor += written;
                        }
                        written
                    }
                }
                VirtualFile::Os { file, .. } => {
                    use std::io::{Read, Seek, SeekFrom};
                    if let Some(pos) = position {
                        if let Err(err) = file.seek(SeekFrom::Start(pos)) {
                            return Err(throw_node_error(vm, "read", None, err));
                        }
                    }
                    let mut read_buf = vec![0u8; max_read];
                    let n = match file.read(&mut read_buf) {
                        Ok(n) => n,
                        Err(err) => return Err(throw_node_error(vm, "read", None, err)),
                    };
                    view.write_bytes(offset, &read_buf[..n])
                }
            };

            Ok(Value::Number(bytes_read as f64))
        }
        Native::NodeFsWriteSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as u32;
            let (to_write_bytes, position) = match arguments.get(1) {
                Some(Value::String(s)) => {
                    let bytes = s.to_lossy_string().into_bytes();
                    let pos = match arguments.get(2) {
                        Some(Value::Number(n)) if *n >= 0.0 => Some(*n as u64),
                        _ => None,
                    };
                    (bytes, pos)
                }
                Some(Value::TypedArray(view)) => {
                    let view_len = view.bytes().len();
                    let offset = arguments.get(2).map_or(0.0, Value::as_number).max(0.0) as usize;
                    let default_len = view_len.saturating_sub(offset);
                    let length = arguments
                        .get(3)
                        .map_or(default_len as f64, Value::as_number)
                        .max(0.0) as usize;
                    let pos = match arguments.get(4) {
                        Some(Value::Number(n)) if *n >= 0.0 => Some(*n as u64),
                        _ => None,
                    };
                    if offset > view_len {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: "offset is out of bounds".to_owned(),
                        });
                    }
                    let max_write = (view_len - offset).min(length);
                    let bytes = view.bytes()[offset..offset + max_write].to_vec();
                    (bytes, pos)
                }
                other => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "The \"buffer\" argument must be an instance of Buffer or TypedArray, or a string, received {}",
                            other.map_or("undefined", Value::type_name)
                        ),
                    });
                }
            };

            let vfile = match vm.fd_table.get_mut(fd) {
                Some(f) => f,
                None => {
                    let err = std::io::Error::from_raw_os_error(9);
                    return Err(throw_node_error(vm, "write", None, err));
                }
            };

            let bytes_written = match vfile {
                VirtualFile::Memory {
                    path,
                    data,
                    cursor,
                    writable,
                    append,
                    ..
                } => {
                    if !*writable {
                        let err = std::io::Error::from_raw_os_error(9);
                        return Err(throw_node_error(vm, "write", None, err));
                    }
                    let write_pos = if *append {
                        data.len()
                    } else if let Some(pos) = position {
                        pos as usize
                    } else {
                        *cursor
                    };
                    let needed = write_pos + to_write_bytes.len();
                    if needed > data.len() {
                        data.resize(needed, 0);
                    }
                    data[write_pos..needed].copy_from_slice(&to_write_bytes);
                    if position.is_none() {
                        *cursor = needed;
                    }
                    let data_clone = data.clone();
                    let path_clone = path.clone();
                    let _ = with_fs(vm, |fs| {
                        fs.write(std::path::Path::new(&path_clone), &data_clone)
                    });
                    to_write_bytes.len()
                }
                VirtualFile::Os { file, .. } => {
                    use std::io::{Seek, SeekFrom, Write};
                    if let Some(pos) = position {
                        if let Err(err) = file.seek(SeekFrom::Start(pos)) {
                            return Err(throw_node_error(vm, "write", None, err));
                        }
                    }
                    match file.write(&to_write_bytes) {
                        Ok(n) => n,
                        Err(err) => return Err(throw_node_error(vm, "write", None, err)),
                    }
                }
            };

            Ok(Value::Number(bytes_written as f64))
        }
        Native::NodeFsFtruncateSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as u32;
            let len = arguments.get(1).map_or(0.0, Value::as_number).max(0.0) as usize;

            let vfile = match vm.fd_table.get_mut(fd) {
                Some(f) => f,
                None => {
                    let err = std::io::Error::from_raw_os_error(9);
                    return Err(throw_node_error(vm, "ftruncate", None, err));
                }
            };

            match vfile {
                VirtualFile::Memory {
                    path,
                    data,
                    cursor,
                    writable,
                    ..
                } => {
                    if !*writable {
                        let err = std::io::Error::from_raw_os_error(9);
                        return Err(throw_node_error(vm, "ftruncate", None, err));
                    }
                    data.resize(len, 0);
                    if *cursor > len {
                        *cursor = len;
                    }
                    let data_clone = data.clone();
                    let path_clone = path.clone();
                    let _ = with_fs(vm, |fs| {
                        fs.write(std::path::Path::new(&path_clone), &data_clone)
                    });
                }
                VirtualFile::Os { file, .. } => {
                    if let Err(err) = file.set_len(len as u64) {
                        return Err(throw_node_error(vm, "ftruncate", None, err));
                    }
                }
            }

            Ok(Value::Undefined)
        }
        Native::NodeFsFsyncSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as u32;
            let vfile = match vm.fd_table.get_mut(fd) {
                Some(f) => f,
                None => {
                    let err = std::io::Error::from_raw_os_error(9);
                    return Err(throw_node_error(vm, "fsync", None, err));
                }
            };

            match vfile {
                VirtualFile::Memory {
                    path,
                    data,
                    writable,
                    ..
                } => {
                    if *writable {
                        let data_clone = data.clone();
                        let path_clone = path.clone();
                        let _ = with_fs(vm, |fs| {
                            fs.write(std::path::Path::new(&path_clone), &data_clone)
                        });
                    }
                }
                VirtualFile::Os { file, .. } => {
                    if let Err(err) = file.sync_all() {
                        return Err(throw_node_error(vm, "fsync", None, err));
                    }
                }
            }

            Ok(Value::Undefined)
        }
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "unsupported synchronous fs operation".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_host_abi::{FileSystemOps, HostError, HostOp, HostOps, HostValue, OverlayFileSystem};
    use kiln_ir::Program;
    use std::collections::HashMap;
    use std::path::PathBuf;

    struct VfsTestHost {
        fs: OverlayFileSystem,
    }

    impl HostOps for VfsTestHost {
        fn call(&mut self, _op: HostOp, _args: &[HostValue]) -> Result<HostValue, HostError> {
            Ok(HostValue::Undefined)
        }

        fn fs(&mut self) -> Option<&mut (dyn FileSystemOps + '_)> {
            Some(&mut self.fs)
        }

        fn clone_fs(&self) -> Option<Box<dyn FileSystemOps>> {
            self.fs.clone_box()
        }
    }

    #[test]
    fn test_virtual_fd_table_lifecycle() {
        let mut table = VirtualFdTable::new();
        let fd1 = table.insert(VirtualFile::Memory {
            path: "/virtual/file1.txt".into(),
            data: b"alpha".to_vec(),
            cursor: 0,
            readable: true,
            writable: false,
            append: false,
        });
        assert!(fd1 >= 10);

        let fd2 = table.insert(VirtualFile::Memory {
            path: "/virtual/file2.txt".into(),
            data: b"beta".to_vec(),
            cursor: 0,
            readable: true,
            writable: true,
            append: false,
        });
        assert_eq!(fd2, fd1 + 1);

        assert!(table.get(fd1).is_some());
        assert!(table.get(fd2).is_some());
        assert!(table.get(999).is_none());

        if let Some(VirtualFile::Memory { cursor, .. }) = table.get_mut(fd1) {
            *cursor = 3;
        }
        if let Some(VirtualFile::Memory { cursor, .. }) = table.get(fd1) {
            assert_eq!(*cursor, 3);
        }

        assert!(table.remove(fd1).is_some());
        assert!(table.get(fd1).is_none());
        assert!(table.remove(fd1).is_none());
    }

    #[test]
    fn test_node_fs_sync_in_memory_vfs_lifecycle() {
        let mut files = HashMap::new();
        let test_path = PathBuf::from("/vfs/sample.txt");
        let content = b"Hello, Virtual VFS World!".to_vec();
        files.insert(test_path.clone(), content.clone());

        let overlay = OverlayFileSystem::new(kiln_host_abi::OsFileSystem, files);
        let mut host = VfsTestHost { fs: overlay };

        let program = Program {
            source: "".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: Vec::new(),
        };

        let mut vm = Vm::with_entropy_seed(
            Rc::new(program.clone()),
            Rc::new(crate::Strictness::sloppy(&program)),
            &mut host,
            vec![],
            crate::ExecutionLimits::default(),
            &crate::RealmCapabilities::default(),
            crate::ExecutionCapabilities::default(),
        )
        .expect("vm init");

        // 1. openSync
        let open_args = vec![
            Value::String(test_path.to_string_lossy().into_owned().into()),
            Value::String("r".into()),
        ];
        let fd_val = dispatch(
            &mut vm,
            Native::NodeFsOpenSync,
            &Value::Undefined,
            &open_args,
            0,
        )
        .expect("openSync succeeds");
        let fd = fd_val.as_number() as u32;
        assert!(fd >= 10, "synthetic FD should start at 10 or above");

        // 2. fstatSync
        let fstat_args = vec![Value::Number(fd as f64)];
        let stat_val = dispatch(
            &mut vm,
            Native::NodeFsFstatSync,
            &Value::Undefined,
            &fstat_args,
            0,
        )
        .expect("fstatSync succeeds");
        let Value::Object(stat_obj) = stat_val else {
            panic!("fstatSync must return an Object");
        };
        let size_val = stat_obj.borrow().get("size").expect("size property");
        assert_eq!(size_val.as_number(), content.len() as f64);

        // 3. readSync chunk 1 (5 bytes)
        let target_buf = node_bytes_value(vec![0u8; 20]);
        let read_args1 = vec![
            Value::Number(fd as f64),
            target_buf.clone(),
            Value::Number(0.0), // offset
            Value::Number(5.0), // length
            Value::Undefined,   // position (sequential cursor)
        ];
        let n1 = dispatch(
            &mut vm,
            Native::NodeFsSync,
            &Value::Undefined,
            &read_args1,
            0,
        )
        .expect("readSync chunk 1 succeeds");
        assert_eq!(n1.as_number(), 5.0);

        let Value::TypedArray(ta) = &target_buf else {
            panic!("must be typed array");
        };
        assert_eq!(&ta.buffer.bytes.borrow()[0..5], b"Hello");

        // 4. readSync chunk 2 (8 bytes, continuing cursor)
        let read_args2 = vec![
            Value::Number(fd as f64),
            target_buf.clone(),
            Value::Number(5.0), // offset in target buffer
            Value::Number(8.0), // length
            Value::Undefined,   // continue cursor
        ];
        let n2 = dispatch(
            &mut vm,
            Native::NodeFsSync,
            &Value::Undefined,
            &read_args2,
            0,
        )
        .expect("readSync chunk 2 succeeds");
        assert_eq!(n2.as_number(), 8.0);
        assert_eq!(&ta.buffer.bytes.borrow()[5..13], b", Virtua");

        // 5. closeSync
        let close_args = vec![Value::Number(fd as f64)];
        let close_res = dispatch(
            &mut vm,
            Native::NodeFsCloseSync,
            &Value::Undefined,
            &close_args,
            0,
        )
        .expect("closeSync succeeds");
        assert!(matches!(close_res, Value::Undefined));

        // 6. Second readSync on closed fd should fail with EBADF
        let err = dispatch(
            &mut vm,
            Native::NodeFsSync,
            &Value::Undefined,
            &read_args1,
            0,
        );
        assert!(err.is_err(), "reading from closed FD must fail");
    }

    #[test]
    fn test_node_fs_sync_os_file_lifecycle() {
        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join(format!("kiln_test_os_file_{}.txt", std::process::id()));
        let content = b"OS File Content Test 12345";
        std::fs::write(&test_path, content).expect("write temp file");

        let files = HashMap::new();
        let overlay = OverlayFileSystem::new(kiln_host_abi::OsFileSystem, files);
        let mut host = VfsTestHost { fs: overlay };

        let program = Program {
            source: "".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: Vec::new(),
        };

        let mut vm = Vm::with_entropy_seed(
            Rc::new(program.clone()),
            Rc::new(crate::Strictness::sloppy(&program)),
            &mut host,
            vec![],
            crate::ExecutionLimits::default(),
            &crate::RealmCapabilities::default(),
            crate::ExecutionCapabilities::default(),
        )
        .expect("vm init");

        let open_args = vec![
            Value::String(test_path.to_string_lossy().into_owned().into()),
            Value::String("r".into()),
        ];
        let fd_val = dispatch(
            &mut vm,
            Native::NodeFsOpenSync,
            &Value::Undefined,
            &open_args,
            0,
        )
        .expect("openSync succeeds for OS file");
        let fd = fd_val.as_number() as u32;
        assert!(fd >= 10);

        let fstat_args = vec![Value::Number(fd as f64)];
        let stat_val = dispatch(
            &mut vm,
            Native::NodeFsFstatSync,
            &Value::Undefined,
            &fstat_args,
            0,
        )
        .expect("fstatSync succeeds for OS file");
        let Value::Object(stat_obj) = stat_val else {
            panic!("fstatSync must return an Object");
        };
        let size_val = stat_obj.borrow().get("size").expect("size property");
        assert_eq!(size_val.as_number(), content.len() as f64);

        let target_buf = node_bytes_value(vec![0u8; 10]);
        let read_args = vec![
            Value::Number(fd as f64),
            target_buf.clone(),
            Value::Number(0.0),
            Value::Number(7.0),
            Value::Undefined,
        ];
        let n = dispatch(
            &mut vm,
            Native::NodeFsSync,
            &Value::Undefined,
            &read_args,
            0,
        )
        .expect("readSync succeeds for OS file");
        assert_eq!(n.as_number(), 7.0);
        let Value::TypedArray(ta) = &target_buf else {
            panic!("typed array");
        };
        assert_eq!(&ta.buffer.bytes.borrow()[0..7], b"OS File");

        let close_args = vec![Value::Number(fd as f64)];
        let close_res = dispatch(
            &mut vm,
            Native::NodeFsCloseSync,
            &Value::Undefined,
            &close_args,
            0,
        )
        .expect("closeSync succeeds for OS file");
        assert!(matches!(close_res, Value::Undefined));

        let _ = std::fs::remove_file(&test_path);
    }

    #[test]
    fn test_node_fs_sync_write_truncate_sync_lifecycle() {
        let mut files = HashMap::new();
        let test_path = PathBuf::from("/vfs/written.txt");
        files.insert(test_path.clone(), Vec::new());

        let overlay = OverlayFileSystem::new(kiln_host_abi::OsFileSystem, files);
        let mut host = VfsTestHost { fs: overlay };

        let program = Program {
            source: "".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: Vec::new(),
        };

        let mut vm = Vm::with_entropy_seed(
            Rc::new(program.clone()),
            Rc::new(crate::Strictness::sloppy(&program)),
            &mut host,
            vec![],
            crate::ExecutionLimits::default(),
            &crate::RealmCapabilities::default(),
            crate::ExecutionCapabilities::default(),
        )
        .expect("vm init");

        // 1. openSync with 'w+'
        let open_args = vec![
            Value::String(test_path.to_string_lossy().into_owned().into()),
            Value::String("w+".into()),
        ];
        let fd_val = dispatch(
            &mut vm,
            Native::NodeFsOpenSync,
            &Value::Undefined,
            &open_args,
            0,
        )
        .expect("openSync w+ succeeds");
        let fd = fd_val.as_number() as u32;

        // 2. writeSync string
        let write_str_args = vec![
            Value::Number(fd as f64),
            Value::String("Hello World".to_string().into()),
        ];
        let written1 = dispatch(
            &mut vm,
            Native::NodeFsWriteSync,
            &Value::Undefined,
            &write_str_args,
            0,
        )
        .expect("writeSync string succeeds");
        assert_eq!(written1.as_number(), 11.0);

        // 3. fstatSync verifies length is 11
        let fstat_args = vec![Value::Number(fd as f64)];
        let stat_val = dispatch(
            &mut vm,
            Native::NodeFsFstatSync,
            &Value::Undefined,
            &fstat_args,
            0,
        )
        .expect("fstatSync succeeds");
        let Value::Object(stat_obj) = stat_val else {
            panic!("Object expected");
        };
        assert_eq!(stat_obj.borrow().get("size").unwrap().as_number(), 11.0);

        // 4. writeSync buffer appending extra bytes
        let buf = node_bytes_value(b" extra".to_vec());
        let write_buf_args = vec![
            Value::Number(fd as f64),
            buf,
            Value::Number(0.0),
            Value::Number(6.0),
            Value::Undefined,
        ];
        let written2 = dispatch(
            &mut vm,
            Native::NodeFsWriteSync,
            &Value::Undefined,
            &write_buf_args,
            0,
        )
        .expect("writeSync buffer succeeds");
        assert_eq!(written2.as_number(), 6.0);

        // 5. ftruncateSync to 5 bytes
        let trunc_args = vec![Value::Number(fd as f64), Value::Number(5.0)];
        let trunc_res = dispatch(
            &mut vm,
            Native::NodeFsFtruncateSync,
            &Value::Undefined,
            &trunc_args,
            0,
        )
        .expect("ftruncateSync succeeds");
        assert!(matches!(trunc_res, Value::Undefined));

        // 6. readSync to verify content is now exactly "Hello"
        let read_buf = node_bytes_value(vec![0u8; 10]);
        let read_args = vec![
            Value::Number(fd as f64),
            read_buf.clone(),
            Value::Number(0.0),
            Value::Number(10.0),
            Value::Number(0.0), // explicit position 0
        ];
        let n = dispatch(
            &mut vm,
            Native::NodeFsSync,
            &Value::Undefined,
            &read_args,
            0,
        )
        .expect("readSync after truncate succeeds");
        assert_eq!(n.as_number(), 5.0);
        let Value::TypedArray(ta) = &read_buf else {
            panic!("typed array");
        };
        assert_eq!(&ta.buffer.bytes.borrow()[0..5], b"Hello");

        // 7. fsyncSync
        let sync_args = vec![Value::Number(fd as f64)];
        let sync_res = dispatch(
            &mut vm,
            Native::NodeFsFsyncSync,
            &Value::Undefined,
            &sync_args,
            0,
        )
        .expect("fsyncSync succeeds");
        assert!(matches!(sync_res, Value::Undefined));

        // 8. fchmodSync and futimesSync
        let chmod_args = vec![Value::Number(fd as f64), Value::Number(0o644 as f64)];
        let chmod_res = dispatch(
            &mut vm,
            Native::NodeFsFchmodSync,
            &Value::Undefined,
            &chmod_args,
            0,
        )
        .expect("fchmodSync succeeds");
        assert!(matches!(chmod_res, Value::Undefined));

        let utimes_args = vec![
            Value::Number(fd as f64),
            Value::Number(1700000000.0),
            Value::Number(1700000000.0),
        ];
        let utimes_res = dispatch(
            &mut vm,
            Native::NodeFsFutimesSync,
            &Value::Undefined,
            &utimes_args,
            0,
        )
        .expect("futimesSync succeeds");
        assert!(matches!(utimes_res, Value::Undefined));

        // Invalid fd throws EBADF
        let bad_chmod_args = vec![Value::Number(999.0), Value::Number(0o644 as f64)];
        assert!(
            dispatch(
                &mut vm,
                Native::NodeFsFchmodSync,
                &Value::Undefined,
                &bad_chmod_args,
                0
            )
            .is_err()
        );

        let bad_utimes_args = vec![
            Value::Number(999.0),
            Value::Number(1000.0),
            Value::Number(2000.0),
        ];
        assert!(
            dispatch(
                &mut vm,
                Native::NodeFsFutimesSync,
                &Value::Undefined,
                &bad_utimes_args,
                0
            )
            .is_err()
        );

        // 9. closeSync
        let close_args = vec![Value::Number(fd as f64)];
        let close_res = dispatch(
            &mut vm,
            Native::NodeFsCloseSync,
            &Value::Undefined,
            &close_args,
            0,
        )
        .expect("closeSync succeeds");
        assert!(matches!(close_res, Value::Undefined));

        // 10. Check VFS backing has "Hello"
        let final_bytes = with_fs(&mut vm, |fs| fs.read(&test_path)).expect("vfs read");
        assert_eq!(final_bytes, b"Hello");
    }
}
