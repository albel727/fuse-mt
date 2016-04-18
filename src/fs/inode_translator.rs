// InodeTranslator :: A wrapper around FUSE that presents paths instead of inodes.
//
// Copyright (c) 2016 by William R. Fraser
//

use std::path::{Path, PathBuf};
use std::rc::Rc;

use fuse::*;
use libc;
use time;

use super::inode_table::*;

pub struct DirectoryEntry {
    pub name: PathBuf,
    pub kind: FileType,
}

pub type ResultEmpty = Result<(), libc::c_int>;
pub type ResultGetattr = Result<(time::Timespec, FileAttr), libc::c_int>;
pub type ResultLookup = Result<(time::Timespec, FileAttr, u64), libc::c_int>;
pub type ResultOpen = Result<(u64, u32), libc::c_int>;
pub type ResultReaddir = Result<Vec<DirectoryEntry>, libc::c_int>;
pub type ResultData = Result<Vec<u8>, libc::c_int>;
pub type ResultWrite = Result<u32, libc::c_int>;

pub trait PathFilesystem {
    fn init(&mut self, _req: &Request) -> ResultEmpty {
        Err(0)
    }

    fn destroy(&mut self, _req: &Request) {
        // Nothing.
    }

    fn getattr(&mut self, _req: &Request, _path: &Path) -> ResultGetattr {
        Err(libc::ENOSYS)
    }

    fn lookup(&mut self, _req: &Request, _parent: &Path, _name: &Path) -> ResultLookup {
        Err(libc::ENOSYS)
    }

    fn opendir(&mut self, _req: &Request, _path: &Path, _flags: u32) -> ResultOpen {
        Err(libc::ENOSYS)
    }

    fn releasedir(&mut self, _req: &Request, _path: &Path, _fh: u64, _flags: u32) -> ResultEmpty {
        Err(libc::ENOSYS)
    }

    fn readdir(&mut self, _req: &Request, _path: &Path, _fh: u64, _offset: u64) -> ResultReaddir {
        Err(libc::ENOSYS)
    }

    fn open(&mut self, _req: &Request, _path: &Path, _flags: u32) -> ResultOpen {
        Err(libc::ENOSYS)
    }

    fn release(&mut self, _req: &Request, _path: &Path, _fh: u64, _flags: u32, _lock_owner: u64, _flush: bool) -> ResultEmpty {
        Err(libc::ENOSYS)
    }

    fn read(&mut self, _req: &Request, _path: &Path, _fh: u64, _offset: u64, _size: u32) -> ResultData {
        Err(libc::ENOSYS)
    }

    fn write(&mut self, _req: &Request, _path: &Path, _fh: u64, _offset: u64, _data: &[u8], _flags: u32) -> ResultWrite {
        Err(libc::ENOSYS)
    }

    fn flush(&mut self, _req: &Request, _path: &Path, _fh: u64, _lock_owner: u64) -> ResultEmpty {
        Err(libc::ENOSYS)
    }
}

pub struct InodeTranslator<T> {
    target: T,
    inodes: InodeTable,
}

impl<T: PathFilesystem> InodeTranslator<T> {
    pub fn new(target_fs: T) -> InodeTranslator<T> {
        let mut translator = InodeTranslator {
            target: target_fs,
            inodes: InodeTable::new()
        };
        translator.inodes.add(Rc::new(PathBuf::from("/")));
        translator
    }
}

impl<T: PathFilesystem> Filesystem for InodeTranslator<T> {
    fn init(&mut self, req: &Request) -> Result<(), libc::c_int> {
        debug!("init");
        self.target.init(req)
    }

    fn destroy(&mut self, req: &Request) {
        debug!("destroy");
        self.target.destroy(req);
    }

    fn getattr(&mut self, req: &Request, ino: u64, reply: ReplyAttr) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("getattr: {:?}", path);
            match self.target.getattr(req, &path) {
                Ok((ref ttl, ref attr)) => reply.attr(ttl, attr),
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL);
        }
    }

    fn lookup(&mut self, req: &Request, parent: u64, name: &Path, reply: ReplyEntry) {
        if let Some(parent_path) = self.inodes.get_path(parent) {
            debug!("lookup: {:?}, {:?}", parent_path, name);
            let path = Rc::new((*parent_path).clone().join(name));
            match self.target.lookup(req, Path::new(&*parent_path), name) {
                Ok((ref ttl, ref mut attr, generation)) => {
                    let ino = self.inodes.add_or_get(path.clone());
                    attr.ino = ino;
                    reply.entry(ttl, attr, generation);
                },
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL);
        }
    }

    fn opendir(&mut self, req: &Request, ino: u64, flags: u32, reply: ReplyOpen) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("opendir: {:?}", path);
            match self.target.opendir(req, &path, flags) {
                Ok((fh, flags)) => reply.opened(fh, flags),
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL);
        }
    }

    fn releasedir(&mut self, req: &Request, ino: u64, fh: u64, flags: u32, reply: ReplyEmpty) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("releasedir: {:?}", path);
            match self.target.releasedir(req, &path, fh, flags) {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL);
        }
    }

    fn readdir(&mut self, req: &Request, ino: u64, fh: u64, offset: u64, mut reply: ReplyDirectory) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("readdir: {:?} @ {}", path, offset);
            match self.target.readdir(req, &path, fh, offset) {
                Ok(entries) => {
                    let parent_inode = if ino == 1 {
                        ino
                    } else {
                        let parent_path: &Path = path.parent().unwrap();
                        match self.inodes.get_inode(parent_path) {
                            Some(inode) => inode,
                            None => {
                                error!("readdir: unable to get inode for parent of {:?}", path);
                                reply.error(libc::EIO);
                                return;
                            }
                        }
                    };

                    let mut index = 0;
                    for entry in entries {
                        let entry_inode = if entry.name == Path::new(".") {
                            ino
                        } else if entry.name == Path::new("..") {
                            parent_inode
                        } else {
                            let path = Rc::new(path.clone().join(&entry.name));
                            self.inodes.add_or_get(path)
                        };

                        let buffer_full: bool = reply.add(
                            entry_inode,
                            index,
                            entry.kind,
                            entry.name.as_os_str());

                        if buffer_full {
                            debug!("readdir: reply buffer is full");
                            break;
                        }

                        index += 1;
                    }

                    reply.ok();
                },
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL);
        }
    }

    fn open(&mut self, req: &Request, ino: u64, flags: u32, reply: ReplyOpen) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("open: {:?}", path);
            match self.target.open(req, &path, flags) {
                Ok((fh, flags)) => reply.opened(fh, flags),
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL);
        }
    }

    fn release(&mut self, req: &Request, ino: u64, fh: u64, flags: u32, lock_owner: u64, flush: bool, reply: ReplyEmpty) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("release: {:?}", path);
            match self.target.release(req, &path, fh, flags, lock_owner, flush) {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL);
        }
    }

    fn read(&mut self, req: &Request, ino: u64, fh: u64, offset: u64, size: u32, reply: ReplyData) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("read: {:?} {:#x} @ {:#x}", path, size, offset);
            match self.target.read(req, &path, fh, offset, size) {
                Ok(ref data) => reply.data(data),
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL);
        }
    }

    fn write(&mut self, req: &Request, ino: u64, fh: u64, offset: u64, data: &[u8], flags: u32, reply: ReplyWrite) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("write: {:?} {:#x} @ {:#x}", path, data.len(), offset);
            match self.target.write(req, &path, fh, offset, data, flags) {
                Ok(written) => reply.written(written),
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL)
        }
    }

    fn flush(&mut self, req: &Request, ino: u64, fh: u64, lock_owner: u64, reply: ReplyEmpty) {
        if let Some(path) = self.inodes.get_path(ino) {
            debug!("flush: {:?}", path);
            match self.target.flush(req, &path, fh, lock_owner) {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EINVAL)
        }
    }
}
