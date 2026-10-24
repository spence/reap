//! Persistent native object identity for declaration adoption.
//!
//! Identity comes from held descriptors and a volume's explicit non-reuse capability.
//! It does not establish scope, policy permission, image uniqueness, or recovery safety.
//! Callers must retain the descriptors and assess those conditions before mutation.

use std::fs::File;
use std::io;

use serde::{Deserialize, Serialize};

/// A volume UUID and its non-recycled native object identifier.
/// Strings preserve the full identifier through JSON clients with narrow integer types.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectIdentity {
  pub volume_uuid: String,
  pub file_id: String,
}

/// Both physical objects that bind a declaration to its containing directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceIdentity {
  pub directory: ObjectIdentity,
  pub declaration: ObjectIdentity,
}

impl InstanceIdentity {
  /// Read both held objects. The caller must establish that `declaration` is the directory's
  /// current regular `.reap` file; descriptor identity alone does not establish that relationship.
  pub fn read(directory: &File, declaration: &File) -> io::Result<Self> {
    if !directory.metadata()?.is_dir() || !declaration.metadata()?.is_file() {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid declaration objects",
      ));
    }
    let directory = ObjectIdentity::read(directory)?;
    let declaration = ObjectIdentity::read(declaration)?;
    if directory.volume_uuid != declaration.volume_uuid {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "declaration crosses volumes",
      ));
    }
    Ok(Self {
      directory,
      declaration,
    })
  }
}

impl ObjectIdentity {
  /// Read a regular file or directory on a volume with proven persistent, non-recycled IDs.
  /// Unsupported platforms and missing capabilities refuse; inode numbers are not a fallback.
  pub fn read(file: &File) -> io::Result<Self> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() && !metadata.is_file() {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "unsupported object type",
      ));
    }
    native::read(file)
  }
}

#[cfg(target_os = "macos")]
mod native {
  use std::ffi::CStr;
  use std::fs::{File, OpenOptions};
  use std::io;
  use std::mem::MaybeUninit;
  use std::os::fd::AsRawFd;
  use std::os::unix::ffi::OsStrExt;
  use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
  use std::path::Path;

  use super::ObjectIdentity;

  const VOLUME_BYTES: usize = 72;
  const OBJECT_BYTES: usize = 32;
  const REQUIRED_CAPS: u32 = libc::VOL_CAP_FMT_PERSISTENTOBJECTIDS
    | libc::VOL_CAP_FMT_PATH_FROM_ID
    | libc::VOL_CAP_FMT_64BIT_OBJECT_IDS;

  pub(super) fn read(file: &File) -> io::Result<ObjectIdentity> {
    let filesystem = filesystem(file)?;
    let mount_end = filesystem
      .f_mntonname
      .iter()
      .position(|b| *b == 0)
      .ok_or_else(|| invalid("unterminated volume path"))?;
    // SAFETY: the bounded array contains a terminating NUL at mount_end.
    let mount = unsafe { CStr::from_ptr(filesystem.f_mntonname.as_ptr()) };
    if mount_end == 0 {
      return Err(invalid("empty volume path"));
    }
    let mount_path = Path::new(std::ffi::OsStr::from_bytes(mount.to_bytes()));
    let volume = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(mount_path)?;
    let volume_fs = self::filesystem(&volume)?;
    if filesystem.f_fsid != volume_fs.f_fsid || file.metadata()?.dev() != volume.metadata()?.dev() {
      return Err(invalid("volume changed during identity assessment"));
    }

    let volume_data = attributes::<VOLUME_BYTES>(
      &volume,
      0,
      libc::ATTR_VOL_INFO | libc::ATTR_VOL_CAPABILITIES | libc::ATTR_VOL_UUID,
    )?;
    let uuid = volume_uuid(&volume_data)?;
    let object_data = attributes::<OBJECT_BYTES>(file, libc::ATTR_CMN_FILEID, 0)?;
    let file_id = object_id(&object_data)?;
    if file_id != file.metadata()?.ino() || filesystem.f_fsid != self::filesystem(file)?.f_fsid {
      return Err(invalid("inconsistent native object identity"));
    }
    Ok(ObjectIdentity {
      volume_uuid: uuid,
      file_id: format!("{file_id:016x}"),
    })
  }

  fn filesystem(file: &File) -> io::Result<libc::statfs> {
    let mut value = MaybeUninit::uninit();
    // SAFETY: file has a live descriptor; value reserves a complete statfs output buffer.
    if unsafe { libc::fstatfs(file.as_raw_fd(), value.as_mut_ptr()) } != 0 {
      return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful fstatfs initializes the complete output structure.
    Ok(unsafe { value.assume_init() })
  }

  fn attributes<const N: usize>(file: &File, common: u32, volume: u32) -> io::Result<[u8; N]> {
    let mut request = libc::attrlist {
      bitmapcount: libc::ATTR_BIT_MAP_COUNT,
      reserved: 0,
      commonattr: libc::ATTR_CMN_RETURNED_ATTRS | common,
      volattr: volume,
      dirattr: 0,
      fileattr: 0,
      forkattr: 0,
    };
    let mut data = [0u8; N];
    // SAFETY: request is initialized; data is a writable N-byte buffer and file remains open.
    if unsafe {
      libc::fgetattrlist(
        file.as_raw_fd(),
        (&mut request as *mut libc::attrlist).cast(),
        data.as_mut_ptr().cast(),
        N,
        0,
      )
    } != 0
    {
      return Err(io::Error::last_os_error());
    }
    Ok(data)
  }

  fn volume_uuid(data: &[u8]) -> io::Result<String> {
    validate(
      data,
      VOLUME_BYTES,
      0,
      libc::ATTR_VOL_CAPABILITIES | libc::ATTR_VOL_UUID,
    )?;
    let capabilities = word(data, 24)?;
    let valid = word(data, 40)?;
    if capabilities & valid & REQUIRED_CAPS != REQUIRED_CAPS {
      return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "volume has no proven non-reuse capability",
      ));
    }
    let uuid = &data[56..72];
    if uuid.iter().all(|b| *b == 0) {
      return Err(invalid("missing volume UUID"));
    }
    let hex: String = uuid.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
      "{}-{}-{}-{}-{}",
      &hex[..8],
      &hex[8..12],
      &hex[12..16],
      &hex[16..20],
      &hex[20..]
    ))
  }

  fn object_id(data: &[u8]) -> io::Result<u64> {
    validate(data, OBJECT_BYTES, libc::ATTR_CMN_FILEID, 0)?;
    let id = u64::from_ne_bytes(data[24..32].try_into().unwrap());
    if id == 0 {
      return Err(invalid("missing native object ID"));
    }
    Ok(id)
  }

  fn validate(data: &[u8], size: usize, common: u32, volume: u32) -> io::Result<()> {
    if data.len() != size || word(data, 0)? as usize != size {
      return Err(invalid("incomplete native attribute buffer"));
    }
    if word(data, 4)? & (libc::ATTR_CMN_RETURNED_ATTRS | common)
      != libc::ATTR_CMN_RETURNED_ATTRS | common
      || word(data, 8)? & volume != volume
    {
      return Err(invalid("missing returned native attributes"));
    }
    Ok(())
  }

  fn word(data: &[u8], offset: usize) -> io::Result<u32> {
    let bytes = data
      .get(offset..offset + 4)
      .ok_or_else(|| invalid("truncated native attributes"))?;
    Ok(u32::from_ne_bytes(bytes.try_into().unwrap()))
  }

  fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
  }

  #[cfg(test)]
  mod tests {
    use super::*;

    fn put(data: &mut [u8], offset: usize, value: u32) {
      data[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }

    #[test]
    fn missing_nonreuse_or_invalid_attributes_cannot_supply_identity() {
      let mut data = [0u8; VOLUME_BYTES];
      put(&mut data, 0, VOLUME_BYTES as u32);
      put(&mut data, 4, libc::ATTR_CMN_RETURNED_ATTRS);
      put(
        &mut data,
        8,
        libc::ATTR_VOL_CAPABILITIES | libc::ATTR_VOL_UUID,
      );
      put(&mut data, 24, REQUIRED_CAPS);
      put(&mut data, 40, REQUIRED_CAPS);
      data[56] = 1;
      assert!(volume_uuid(&data).is_ok());
      for offset in [24, 40] {
        for bit in [
          libc::VOL_CAP_FMT_PERSISTENTOBJECTIDS,
          libc::VOL_CAP_FMT_PATH_FROM_ID,
          libc::VOL_CAP_FMT_64BIT_OBJECT_IDS,
        ] {
          let mut missing = data;
          put(&mut missing, offset, REQUIRED_CAPS & !bit);
          assert_eq!(
            volume_uuid(&missing).unwrap_err().kind(),
            io::ErrorKind::Unsupported
          );
        }
      }
      for offset in [0, 4, 8] {
        let mut missing = data;
        put(&mut missing, offset, 0);
        assert!(volume_uuid(&missing).is_err());
      }
      assert!(volume_uuid(&data[..VOLUME_BYTES - 1]).is_err());
      data[56] = 0;
      assert!(volume_uuid(&data).is_err());
    }

    #[test]
    fn full_width_object_ids_require_complete_returned_attributes() {
      let mut data = [0u8; OBJECT_BYTES];
      put(&mut data, 0, OBJECT_BYTES as u32);
      put(
        &mut data,
        4,
        libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_FILEID,
      );
      data[24..32].copy_from_slice(&u64::MAX.to_ne_bytes());
      assert_eq!(object_id(&data).unwrap(), u64::MAX);
      assert!(object_id(&data[..OBJECT_BYTES - 1]).is_err());
      put(&mut data, 4, libc::ATTR_CMN_RETURNED_ATTRS);
      assert!(object_id(&data).is_err());
      put(
        &mut data,
        4,
        libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_FILEID,
      );
      data[24..32].fill(0);
      assert!(object_id(&data).is_err());
    }
  }
}

#[cfg(not(target_os = "macos"))]
mod native {
  use super::*;

  pub(super) fn read(_: &File) -> io::Result<ObjectIdentity> {
    Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "persistent disposal identity is unavailable",
    ))
  }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
  use std::fs::{self, File, OpenOptions};
  use std::os::unix::fs::{symlink, OpenOptionsExt};
  use std::path::{Path, PathBuf};
  use std::sync::atomic::{AtomicUsize, Ordering};

  use super::*;

  static NEXT: AtomicUsize = AtomicUsize::new(0);

  struct Fixture(PathBuf);

  impl Fixture {
    fn new() -> Self {
      let base = std::env::var_os("REAP_IDENTITY_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
      let path = base.join(format!(
        "identity-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
      ));
      fs::create_dir(&path).unwrap();
      Self(path)
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }

  fn instance(path: &Path) -> InstanceIdentity {
    InstanceIdentity::read(
      &File::open(path).unwrap(),
      &File::open(path.join(".reap")).unwrap(),
    )
    .unwrap()
  }

  #[test]
  fn native_rename_copy_replacement_and_recreation() {
    let fixture = Fixture::new();
    let original = fixture.0.join("original");
    fs::create_dir(&original).unwrap();
    fs::write(original.join(".reap"), b"identity fixture").unwrap();
    let initial = instance(&original);
    let renamed = fixture.0.join("renamed");
    fs::rename(&original, &renamed).unwrap();
    assert_eq!(initial, instance(&renamed));

    let copy = fixture.0.join("copy");
    fs::create_dir(&copy).unwrap();
    fs::copy(renamed.join(".reap"), copy.join(".reap")).unwrap();
    let copied = instance(&copy);
    assert_ne!(initial.directory, copied.directory);
    assert_ne!(initial.declaration, copied.declaration);
    fs::write(renamed.join("replacement"), b"identity fixture").unwrap();
    fs::rename(renamed.join("replacement"), renamed.join(".reap")).unwrap();
    let replaced = instance(&renamed);
    assert_eq!(initial.directory, replaced.directory);
    assert_ne!(initial.declaration, replaced.declaration);

    fs::remove_file(renamed.join(".reap")).unwrap();
    fs::remove_dir(&renamed).unwrap();
    fs::create_dir(&renamed).unwrap();
    fs::write(renamed.join(".reap"), b"identity fixture").unwrap();
    let recreated = instance(&renamed);
    assert_ne!(initial.directory, recreated.directory);
    assert_ne!(replaced.declaration, recreated.declaration);
    println!(
      "native identity fixture: {}",
      serde_json::to_string(&recreated).unwrap()
    );
  }

  #[test]
  fn native_clone_does_not_inherit_declaration_identity() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    let copy = fixture.0.join("clone");
    fs::write(&source, b"native clone fixture").unwrap();
    let from = std::ffi::CString::new(source.as_os_str().as_encoded_bytes()).unwrap();
    let to = std::ffi::CString::new(copy.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: both C strings remain valid for the call; the destination is an unused owned path.
    assert_eq!(unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), 0) }, 0);
    assert_ne!(
      ObjectIdentity::read(&File::open(source).unwrap()).unwrap(),
      ObjectIdentity::read(&File::open(copy).unwrap()).unwrap()
    );
  }

  #[test]
  fn invalid_types_and_symlink_descriptors_refuse() {
    let fixture = Fixture::new();
    let file = fixture.0.join("file");
    fs::write(&file, b"fixture").unwrap();
    let file = File::open(file).unwrap();
    let directory = File::open(&fixture.0).unwrap();
    assert!(InstanceIdentity::read(&file, &directory).is_err());
    assert!(InstanceIdentity::read(&directory, &directory).is_err());
    symlink("file", fixture.0.join("link")).unwrap();
    let link = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_SYMLINK)
      .open(fixture.0.join("link"))
      .unwrap();
    assert!(ObjectIdentity::read(&link).is_err());
    assert!(InstanceIdentity::read(&directory, &link).is_err());
    assert!(ObjectIdentity::read(&File::open("/dev/null").unwrap()).is_err());
  }

  #[test]
  fn held_descriptor_cannot_switch_to_a_replacement() {
    let fixture = Fixture::new();
    let path = fixture.0.join("object");
    fs::write(&path, b"original").unwrap();
    let held = File::open(&path).unwrap();
    let initial = ObjectIdentity::read(&held).unwrap();
    fs::write(fixture.0.join("replacement"), b"replacement").unwrap();
    fs::rename(fixture.0.join("replacement"), &path).unwrap();
    assert_eq!(initial, ObjectIdentity::read(&held).unwrap());
    assert_ne!(
      initial,
      ObjectIdentity::read(&File::open(&path).unwrap()).unwrap()
    );
  }
}
