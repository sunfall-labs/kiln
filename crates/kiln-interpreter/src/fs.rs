//! `node:fs` and `node:fs/promises` implementation.
//!
//! Provides filesystem operation bindings, options parsing, and property lookups.

use std::cell::RefCell;
use std::rc::Rc;

use crate::buffer::node_bytes_value;
use crate::{ArrayData, ExecutionError, HostOps, Native, ObjectData, RealmId, Value, Vm};

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
        "openSync" => Some(Value::Native(Native::NodeFsOpenSync, RealmId::PRIMARY)),
        "closeSync" => Some(Value::Native(Native::NodeFsCloseSync, RealmId::PRIMARY)),
        "readlinkSync" => Some(Value::Native(Native::NodeFsReadlinkSync, RealmId::PRIMARY)),
        "rmdirSync" => Some(Value::Native(Native::NodeFsRmdirSync, RealmId::PRIMARY)),
        "utimesSync" => Some(Value::Native(Native::NodeFsUtimesSync, RealmId::PRIMARY)),
        "readSync" => Some(Value::Native(Native::NodeFsSync, RealmId::PRIMARY)),
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
        Native::NodeFsOpenSync => Some("openSync"),
        Native::NodeFsCloseSync => Some("closeSync"),
        Native::NodeFsReadlinkSync => Some("readlinkSync"),
        Native::NodeFsRmdirSync => Some("rmdirSync"),
        Native::NodeFsUtimesSync => Some("utimesSync"),
        Native::NodeFsReadFile => Some("readFile"),
        Native::NodeFsWriteFile => Some("writeFile"),
        Native::NodeFsAppendFile => Some("appendFile"),
        Native::NodeFsStat => Some("stat"),
        Native::NodeFsMkdir => Some("mkdir"),
        Native::NodeStatIsFile => Some("isFile"),
        Native::NodeStatIsDirectory => Some("isDirectory"),
        Native::NodeStatIsSymbolicLink => Some("isSymbolicLink"),
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
        code: code.to_owned(),
        message,
        syscall: syscall.to_owned(),
        path,
    };
    let err_val = vm.node_error_value(error);
    vm.caught = Some(err_val);
    ExecutionError::ThrownAcrossNative
}

pub(crate) fn stats_object(meta: &std::fs::Metadata) -> Value {
    let mut object = ObjectData::default();
    let is_file = meta.is_file();
    let is_dir = meta.is_dir();
    let is_symlink = meta.file_type().is_symlink();
    let size = meta.len() as f64;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_millis() as f64);
    let birthtime_ms = meta
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(mtime_ms, |d| d.as_millis() as f64);
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() as f64
    };
    #[cfg(not(unix))]
    let mode = 0.0;

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
            let exists = !path_str.is_empty() && std::path::Path::new(&path_str).exists();
            Ok(Value::Boolean(exists))
        }
        Native::NodeFsReadFileSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            match std::fs::read(&path_str) {
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
            if let Err(err) = std::fs::write(&path_str, data) {
                return Err(throw_node_error(vm, "open", Some(path_str), err));
            }
            let mode = match arguments.get(2) {
                Some(Value::Number(n)) => Some(*n as u32),
                Some(Value::Object(obj)) => obj.borrow().get_own("mode").and_then(|v| match v {
                    Value::Number(n) => Some(*n as u32),
                    _ => None,
                }),
                _ => None,
            };
            #[cfg(unix)]
            if let Some(mode) = mode {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path_str, std::fs::Permissions::from_mode(mode));
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
            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path_str);
            let mut file = match file {
                Ok(f) => f,
                Err(err) => return Err(throw_node_error(vm, "open", Some(path_str), err)),
            };
            if let Err(err) = file.write_all(&data) {
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
            let res = if recursive {
                std::fs::create_dir_all(&path_str)
            } else {
                std::fs::create_dir(&path_str)
            };
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
            let meta_res = if is_lstat {
                std::fs::symlink_metadata(&path_str)
            } else {
                std::fs::metadata(&path_str)
            };
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
            let fd = arguments.first().map_or(0.0, Value::as_number) as i32;
            #[cfg(unix)]
            {
                use std::os::unix::io::FromRawFd;
                use std::os::unix::io::IntoRawFd;
                if fd >= 0 {
                    let file = unsafe { std::fs::File::from_raw_fd(fd) };
                    let meta_res = file.metadata();
                    let _ = file.into_raw_fd();
                    match meta_res {
                        Ok(meta) => Ok(stats_object(&meta)),
                        Err(err) => Err(throw_node_error(vm, "fstat", None, err)),
                    }
                } else {
                    let err = std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "bad file descriptor",
                    );
                    Err(throw_node_error(vm, "fstat", None, err))
                }
            }
            #[cfg(not(unix))]
            {
                let _ = fd;
                Ok(Value::Undefined)
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
            let read_dir = match std::fs::read_dir(&path_str) {
                Ok(rd) => rd,
                Err(err) => return Err(throw_node_error(vm, "scandir", Some(path_str), err)),
            };
            let mut entries = Vec::new();
            for entry in read_dir {
                let entry = match entry {
                    Ok(e) => e,
                    Err(err) => return Err(throw_node_error(vm, "scandir", Some(path_str), err)),
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                if with_file_types {
                    let file_type = entry.file_type().ok();
                    let is_file = file_type.map_or(false, |ft| ft.is_file());
                    let is_dir = file_type.map_or(false, |ft| ft.is_dir());
                    let is_symlink = file_type.map_or(false, |ft| ft.is_symlink());
                    let mut dirent = ObjectData::default();
                    dirent.set("name", Value::String(name.into()));
                    dirent.set_non_enumerable("@@kiln:node:is-file", Value::Boolean(is_file));
                    dirent.set_non_enumerable("@@kiln:node:is-directory", Value::Boolean(is_dir));
                    dirent.set_non_enumerable("@@kiln:node:is-symlink", Value::Boolean(is_symlink));
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
                    entries.push(Value::Object(Rc::new(RefCell::new(dirent))));
                } else {
                    entries.push(Value::String(name.into()));
                }
            }
            Ok(Value::Array(ArrayData::new(entries)))
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
            if !path.exists() {
                if force {
                    return Ok(Value::Undefined);
                }
                let err =
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
                return Err(throw_node_error(vm, "rm", Some(path_str), err));
            }
            let res = if path.is_dir() {
                if recursive {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_dir(path)
                }
            } else {
                std::fs::remove_file(path)
            };
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
            if let Err(err) = std::fs::remove_file(&path_str) {
                return Err(throw_node_error(vm, "unlink", Some(path_str), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsRealpathSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            match std::fs::canonicalize(&path_str) {
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
            if let Err(err) = std::fs::rename(&old_path, &new_path) {
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
            if let Err(err) = std::fs::copy(&src, &dest) {
                return Err(throw_node_error(vm, "copyfile", Some(src), err));
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsAccessSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            if !std::path::Path::new(&path_str).exists() {
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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(err) =
                    std::fs::set_permissions(&path_str, std::fs::Permissions::from_mode(mode))
                {
                    return Err(throw_node_error(vm, "chmod", Some(path_str), err));
                }
            }
            #[cfg(not(unix))]
            {
                let _ = (path_str, mode);
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
            let mut opts = std::fs::OpenOptions::new();
            match flags.as_str() {
                "r" | "rs" => {
                    opts.read(true);
                }
                "r+" | "rs+" => {
                    opts.read(true).write(true);
                }
                "w" => {
                    opts.write(true).create(true).truncate(true);
                }
                "wx" | "xw" => {
                    opts.write(true).create_new(true);
                }
                "w+" => {
                    opts.read(true).write(true).create(true).truncate(true);
                }
                "wx+" | "xw+" => {
                    opts.read(true).write(true).create_new(true);
                }
                "a" => {
                    opts.append(true).create(true);
                }
                "ax" | "xa" => {
                    opts.append(true).create_new(true);
                }
                "a+" => {
                    opts.read(true).append(true).create(true);
                }
                "ax+" | "xa+" => {
                    opts.read(true).append(true).create_new(true);
                }
                _ => {
                    opts.read(true);
                }
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
            #[cfg(unix)]
            {
                use std::os::unix::io::IntoRawFd;
                let fd = file.into_raw_fd();
                Ok(Value::Number(fd as f64))
            }
            #[cfg(not(unix))]
            {
                let _ = file;
                Ok(Value::Number(3.0))
            }
        }
        Native::NodeFsCloseSync => {
            let fd = arguments.first().map_or(0.0, Value::as_number) as i32;
            #[cfg(unix)]
            {
                use std::os::unix::io::FromRawFd;
                if fd >= 0 {
                    let _ = unsafe { std::fs::File::from_raw_fd(fd) };
                }
            }
            #[cfg(not(unix))]
            {
                let _ = fd;
            }
            Ok(Value::Undefined)
        }
        Native::NodeFsReadlinkSync => {
            let path_str = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            match std::fs::read_link(&path_str) {
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
            let res = if recursive {
                std::fs::remove_dir_all(&path_str)
            } else {
                std::fs::remove_dir(&path_str)
            };
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
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "unsupported synchronous fs operation".to_owned(),
        }),
    }
}
