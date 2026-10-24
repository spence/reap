//! Native physical resource keys for worker reservations.
//!
//! Resolution is read-only and synchronous. Run it in the unknown-volume resolver worker,
//! never in the control loop. Retain the input descriptor and resolve again before a grant.
//! Keys include native boot identity. Each key identifies an Apple Fabric controller or
//! a tunneled PCIe NVMe endpoint.
//! Other storage topologies refuse until their complete native mappings have proof.

use std::fs::File;
use std::io;

/// Resolve a held regular file or directory to sorted, unique physical resource keys.
///
/// macOS supports local APFS with one physical store, a direct GUID partition, and an
/// internal Apple Fabric NVMe controller or an external tunneled PCIe NVMe endpoint.
/// Other layouts refuse.
/// Two complete observations must agree. This detects observed changes, not changes after
/// return or an unobserved change-and-revert. Callers must retain reservations through recovery
/// and revalidate keys before a grant. Keys include `crate::launch::boot_id()`.
/// They are not persistent disk identities.
/// No helper, target write, device open, serial-number query, or credential change occurs.
#[cfg(target_os = "macos")]
pub fn resolve(file: &File) -> io::Result<Vec<String>> {
  native::resolve(file)
}

/// Refuse platforms without native physical topology proof.
#[cfg(not(target_os = "macos"))]
pub fn resolve(_file: &File) -> io::Result<Vec<String>> {
  Err(io::Error::new(
    io::ErrorKind::Unsupported,
    "native physical resource resolution is unsupported on this platform",
  ))
}

#[cfg(target_os = "macos")]
mod native {
  use std::ffi::{c_char, c_void, CStr};
  use std::fs::File;
  use std::io;
  use std::mem::MaybeUninit;
  use std::os::fd::AsRawFd;
  use std::ptr;

  const MAX_NODES: usize = 16;
  const MAX_TEXT: usize = 128;
  const UTF8: u32 = 0x08000100;
  // IONVMeFamily matches PCI class 0x010802; the registry publishes four little-endian bytes.
  const NVME_PCI_CLASS: [u8; 4] = [2, 8, 1, 0];
  type CfRef = *const c_void;

  #[derive(Clone, Debug, Eq, PartialEq)]
  struct Mount {
    device: u64,
    fsid: [i32; 2],
    source: Vec<u8>,
  }

  #[derive(Clone, Debug, Eq, PartialEq)]
  struct Node {
    id: u64,
    class: String,
    whole: Option<bool>,
    protocol: Option<(String, String)>,
    controller: bool,
    tunnel_compatible: Option<bool>,
    pci_class: Option<[u8; 4]>,
    tunnelled: Option<bool>,
  }

  #[derive(Clone, Debug, Eq, PartialEq)]
  struct Observation {
    boot: String,
    mount: Mount,
    nodes: Vec<Node>,
  }

  pub(super) fn resolve(file: &File) -> io::Result<Vec<String>> {
    stable(|| observe(file))
  }

  fn stable(mut observe: impl FnMut() -> io::Result<Observation>) -> io::Result<Vec<String>> {
    let first = observe()?;
    let key = resource(&first)?;
    let second = observe()?;
    resource(&second)?;
    if first != second {
      return Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "physical resource mapping changed during resolution",
      ));
    }
    Ok(vec![key])
  }

  /// Admit only the complete topology established by native source and host proof.
  /// Apple documents the APFS provider graph in `BLAPFSCreatePhysicalStoreBSDsFromVolumeBSD`:
  /// <https://github.com/apple-oss-distributions/bless/blob/main/libbless/APFS/BLAPFSUtilities.c>.
  /// `BLAPFSSnapshotToVolume` in that source also establishes snapshot ancestry.
  fn resource(observation: &Observation) -> io::Result<String> {
    let nodes = &observation.nodes;
    if nodes.len() > MAX_NODES || nodes.iter().any(|n| n.id == 0) {
      return Err(unsupported("invalid or excessive registry nodes"));
    }
    for (index, node) in nodes.iter().enumerate() {
      if nodes[..index].iter().any(|previous| previous.id == node.id) {
        return Err(unsupported("registry cycle or duplicate node"));
      }
    }
    let nodes = if nodes
      .first()
      .is_some_and(|n| n.class == "AppleAPFSSnapshot")
    {
      if nodes[0].whole != Some(false) {
        return Err(unsupported("invalid APFS snapshot media"));
      }
      &nodes[1..]
    } else {
      nodes.as_slice()
    };
    let classes = [
      "AppleAPFSVolume",
      "AppleAPFSContainer",
      "AppleAPFSMedia",
      "AppleAPFSContainerScheme",
      "IOMedia",
      "IOGUIDPartitionScheme",
      "IOMedia",
      "IOBlockStorageDriver",
    ];
    if ![10, 11].contains(&nodes.len())
      || nodes
        .iter()
        .zip(classes)
        .any(|(node, class)| node.class != class)
      || nodes[0].whole != Some(false)
      || nodes[2].whole != Some(true)
      || nodes[4].whole != Some(false)
      || nodes[6].whole != Some(true)
      || !nodes[9].controller
    {
      return Err(unsupported("physical storage topology is not proved"));
    }
    let protocol = nodes[8]
      .protocol
      .as_ref()
      .map(|(bus, location)| (bus.as_str(), location.as_str()));
    let (kind, endpoint) = match (nodes[8].class.as_str(), protocol, nodes.len()) {
      ("IOEmbeddedNVMeBlockDevice", Some(("Apple Fabric", "Internal")), 10) => {
        ("fabric", nodes[9].id)
      }
      ("IONVMeBlockStorageDevice", Some(("PCI-Express", "External")), 11)
        if nodes[9].class == "IONVMeController"
          && nodes[9].tunnel_compatible == Some(true)
          && nodes[10].class == "IOPCIDevice"
          && nodes[10].pci_class == Some(NVME_PCI_CLASS)
          && nodes[10].tunnelled == Some(true) =>
      {
        ("pcie", nodes[10].id)
      }
      _ => return Err(unsupported("physical NVMe endpoint is not proved")),
    };
    if observation.boot.is_empty()
      || observation.boot.len() > 64
      || !observation
        .boot
        .bytes()
        .all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
      return Err(unsupported(
        "native boot identity exceeds the resource key contract",
      ));
    }
    Ok(format!(
      "macos-nvme:{}:{kind}:{endpoint:016x}",
      observation.boot
    ))
  }

  fn observe(file: &File) -> io::Result<Observation> {
    let mount = mount(file)?;
    let boot = crate::launch::boot_id()?;
    let name = CStr::from_bytes_with_nul(&mount.source)
      .map_err(|_| unsupported("invalid native mount source"))?;
    let name = name.to_bytes();
    let bsd = name
      .strip_prefix(b"/dev/")
      .ok_or_else(|| unsupported("non-device mount"))?;
    if bsd.is_empty() || bsd.len() >= MAX_TEXT || !bsd.iter().all(u8::is_ascii_alphanumeric) {
      return Err(unsupported("invalid native BSD device name"));
    }
    let mut bsd_name = [0; MAX_TEXT];
    bsd_name[..bsd.len()].copy_from_slice(bsd);
    // SAFETY: bsd_name is bounded and NUL-terminated. IOKit consumes the matching dictionary.
    let mut current = unsafe {
      let matching = IOBSDNameMatching(0, 0, bsd_name.as_ptr().cast());
      if matching.is_null() {
        return Err(unsupported("native device matching is unavailable"));
      }
      Io::new(IOServiceGetMatchingService(0, matching))?
    };
    let major = number(&current, c"BSD Major")?;
    let minor = number(&current, c"BSD Minor")?;
    if major != i64::from(libc::major(mount.device as libc::dev_t))
      || minor != i64::from(libc::minor(mount.device as libc::dev_t))
    {
      return Err(unsupported("held descriptor and native media disagree"));
    }

    let mut nodes = Vec::with_capacity(MAX_NODES);
    loop {
      if nodes.len() == MAX_NODES {
        return Err(unsupported("registry traversal exceeds its bound"));
      }
      let node = current.read()?;
      let terminal =
        node.class == "IOPCIDevice" || (node.controller && node.class != "IONVMeController");
      nodes.push(node);
      if terminal {
        break;
      }
      current = current.parent()?;
    }
    // the same held object must still describe the same mount after registry traversal
    if mount != self::mount(file)? {
      return Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "native mount mapping changed",
      ));
    }
    Ok(Observation { boot, mount, nodes })
  }

  fn mount(file: &File) -> io::Result<Mount> {
    let metadata = file.metadata()?;
    if !metadata.is_file() && !metadata.is_dir() {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "resource input is not a file or directory",
      ));
    }
    let mut fs = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: the descriptor is held; fstatfs initializes fs on success.
    if unsafe { libc::fstatfs(file.as_raw_fd(), fs.as_mut_ptr()) } != 0 {
      return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatfs succeeded and initialized the structure.
    let fs = unsafe { fs.assume_init() };
    if fs.f_flags & libc::MNT_LOCAL as u32 == 0 || bytes(&fs.f_fstypename)? != b"apfs\0" {
      return Err(unsupported("only local APFS has physical topology proof"));
    }
    // APFS snapshots and firmlinks can make st_dev differ from the mounted media device.
    // DiskArbitration's DADiskCreateFromVolumePath uses f_fsid.val[0] for BSD major/minor.
    // <https://github.com/apple-oss-distributions/DiskArbitration/blob/main/diskarbitrationd/DADisk.c>
    // SAFETY: Darwin fsid_t is two int32_t fields; transmute also verifies the size at compile time.
    let fsid: [i32; 2] = unsafe { std::mem::transmute(fs.f_fsid) };
    Ok(Mount {
      device: fsid[0] as u32 as u64,
      fsid,
      source: bytes(&fs.f_mntfromname)?,
    })
  }

  fn bytes(value: &[c_char]) -> io::Result<Vec<u8>> {
    let end = value
      .iter()
      .position(|b| *b == 0)
      .ok_or_else(|| unsupported("unterminated native mount field"))?;
    Ok(value[..=end].iter().map(|b| *b as u8).collect())
  }

  /// Own one IOKit object reference, including iterator references.
  struct Io(u32);

  impl Io {
    fn new(value: u32) -> io::Result<Self> {
      if value == 0 {
        Err(unsupported("native registry object is unavailable"))
      } else {
        Ok(Self(value))
      }
    }

    fn read(&self) -> io::Result<Node> {
      let mut class = [0; MAX_TEXT];
      let mut id = 0;
      // SAFETY: self owns a live reference; class and id are writable fixed-size native outputs.
      if unsafe { IOObjectGetClass(self.0, class.as_mut_ptr()) } != 0
        || unsafe { IORegistryEntryGetRegistryEntryID(self.0, &mut id) } != 0
      {
        return Err(unsupported("native registry identity is unavailable"));
      }
      let class = bytes(&class)?;
      let class = String::from_utf8(class[..class.len() - 1].to_vec()).map_err(io::Error::other)?;
      // SAFETY: self owns a live reference and both class names are static NUL-terminated strings.
      let media = unsafe { IOObjectConformsTo(self.0, c"IOMedia".as_ptr()) } != 0;
      // SAFETY: the object is live and the class name is NUL-terminated.
      let controller = unsafe { IOObjectConformsTo(self.0, c"IONVMeController".as_ptr()) } != 0;
      let whole = if media {
        Some(boolean(&property(self, c"Whole")?)?)
      } else {
        None
      };
      let protocol = if matches!(
        class.as_str(),
        "IOEmbeddedNVMeBlockDevice" | "IONVMeBlockStorageDevice"
      ) {
        let properties = property(self, c"Protocol Characteristics")?;
        // SAFETY: properties is a live CF object; no cast occurs until its type is verified.
        if unsafe { CFGetTypeID(properties.0) != CFDictionaryGetTypeID() } {
          return Err(unsupported("invalid storage protocol properties"));
        }
        Some((
          dictionary_text(&properties, c"Physical Interconnect")?,
          dictionary_text(&properties, c"Physical Interconnect Location")?,
        ))
      } else {
        None
      };
      let tunnel_compatible = if class == "IONVMeController" {
        Some(boolean(&property(self, c"IOPCITunnelCompatible")?)?)
      } else {
        None
      };
      let (pci_class, tunnelled) = if class == "IOPCIDevice" {
        (
          Some(data4(&property(self, c"class-code")?)?),
          Some(boolean(&property(self, c"IOPCITunnelled")?)?),
        )
      } else {
        (None, None)
      };
      Ok(Node {
        id,
        class,
        whole,
        protocol,
        controller,
        tunnel_compatible,
        pci_class,
        tunnelled,
      })
    }

    /// Require a complete, valid parent iterator with exactly one provider.
    fn parent(&self) -> io::Result<Self> {
      let mut iterator = 0;
      // SAFETY: self owns a live reference; iterator receives an owned native reference on success.
      if unsafe { IORegistryEntryGetParentIterator(self.0, c"IOService".as_ptr(), &mut iterator) }
        != 0
      {
        return Err(unsupported("native parent enumeration failed"));
      }
      let iterator = Io::new(iterator)?;
      // SAFETY: iterator owns a live native iterator. Each nonzero result transfers one reference.
      let first = Io::new(unsafe { IOIteratorNext(iterator.0) })?;
      // SAFETY: iterator is live. The second result is released even when the topology refuses.
      let second = unsafe { IOIteratorNext(iterator.0) };
      if second != 0 {
        drop(Io(second));
        return Err(unsupported(
          "multi-provider storage topology is unsupported",
        ));
      }
      // SAFETY: the live iterator reached its end; validity establishes complete enumeration.
      if unsafe { IOIteratorIsValid(iterator.0) } == 0 {
        return Err(unsupported("native parent enumeration is incomplete"));
      }
      Ok(first)
    }
  }

  impl Drop for Io {
    fn drop(&mut self) {
      // SAFETY: self owns exactly one nonzero native reference.
      unsafe {
        IOObjectRelease(self.0);
      }
    }
  }

  /// Own one CoreFoundation create/copy reference.
  struct Cf(CfRef);

  impl Drop for Cf {
    fn drop(&mut self) {
      // SAFETY: self owns one nonnull CoreFoundation reference.
      unsafe {
        CFRelease(self.0);
      }
    }
  }

  fn cf_text(value: &CStr) -> io::Result<Cf> {
    // SAFETY: value is NUL-terminated and all call sites supply ASCII property names.
    let value = unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), UTF8) };
    if value.is_null() {
      Err(unsupported("native property key is unavailable"))
    } else {
      Ok(Cf(value))
    }
  }

  fn property(node: &Io, key: &CStr) -> io::Result<Cf> {
    let key = cf_text(key)?;
    // SAFETY: node and key own live references. The result has create ownership.
    let value = unsafe { IORegistryEntryCreateCFProperty(node.0, key.0, ptr::null(), 0) };
    if value.is_null() {
      Err(unsupported("required native property is unavailable"))
    } else {
      Ok(Cf(value))
    }
  }

  fn dictionary_text(dictionary: &Cf, key: &CStr) -> io::Result<String> {
    let key = cf_text(key)?;
    // SAFETY: the caller verified dictionary's type. key is live; the borrowed value stays live.
    let value = unsafe { CFDictionaryGetValue(dictionary.0, key.0) };
    if value.is_null() {
      return Err(unsupported(
        "required native protocol property is unavailable",
      ));
    }
    // SAFETY: value is a live borrowed object. Type verification precedes the string API.
    if unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
      return Err(unsupported("invalid native protocol property type"));
    }
    let mut bytes = [0i8; MAX_TEXT];
    // SAFETY: value is a CFString and bytes is a fixed-size writable buffer.
    if unsafe { CFStringGetCString(value, bytes.as_mut_ptr(), bytes.len() as isize, UTF8) } == 0 {
      return Err(unsupported("native protocol property exceeds its bound"));
    }
    let bytes = self::bytes(&bytes)?;
    String::from_utf8(bytes[..bytes.len() - 1].to_vec()).map_err(io::Error::other)
  }

  /// Read exactly one PCI class-code property without unrelated device properties.
  fn data4(value: &Cf) -> io::Result<[u8; 4]> {
    // SAFETY: value is live; its CFData type is verified before the CFData length API.
    if unsafe { CFGetTypeID(value.0) != CFDataGetTypeID() || CFDataGetLength(value.0) != 4 } {
      return Err(unsupported("invalid native PCI class-code property"));
    }
    // SAFETY: value is verified CFData with four bytes; its borrowed storage stays live.
    let bytes = unsafe { CFDataGetBytePtr(value.0) };
    if bytes.is_null() {
      return Err(unsupported("native PCI class-code storage is unavailable"));
    }
    let mut result = [0; 4];
    // SAFETY: bytes references four live bytes and result has four writable, nonoverlapping bytes.
    unsafe {
      ptr::copy_nonoverlapping(bytes, result.as_mut_ptr(), result.len());
    }
    Ok(result)
  }

  fn boolean(value: &Cf) -> io::Result<bool> {
    // SAFETY: value is live; type verification precedes the CFBoolean API.
    if unsafe { CFGetTypeID(value.0) != CFBooleanGetTypeID() } {
      return Err(unsupported("invalid native boolean property"));
    }
    // SAFETY: value has the verified CFBoolean type.
    Ok(unsafe { CFBooleanGetValue(value.0) } != 0)
  }

  fn number(node: &Io, key: &CStr) -> io::Result<i64> {
    let value = property(node, key)?;
    let mut number = 0i64;
    // SAFETY: value is live; type verification precedes CFNumberGetValue with a matching i64 buffer.
    if unsafe {
      CFGetTypeID(value.0) != CFNumberGetTypeID()
        || CFNumberGetValue(value.0, 4, (&mut number as *mut i64).cast()) == 0
    } {
      return Err(unsupported("invalid native device number"));
    }
    Ok(number)
  }

  fn unsupported(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
  }

  #[link(name = "IOKit", kind = "framework")]
  extern "C" {
    fn IOBSDNameMatching(port: u32, options: u32, name: *const c_char) -> CfRef;
    fn IOServiceGetMatchingService(port: u32, matching: CfRef) -> u32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOObjectGetClass(object: u32, class: *mut c_char) -> i32;
    fn IOObjectConformsTo(object: u32, class: *const c_char) -> u32;
    fn IORegistryEntryGetRegistryEntryID(entry: u32, id: *mut u64) -> i32;
    fn IORegistryEntryCreateCFProperty(
      entry: u32,
      key: CfRef,
      allocator: CfRef,
      options: u32,
    ) -> CfRef;
    fn IORegistryEntryGetParentIterator(
      entry: u32,
      plane: *const c_char,
      iterator: *mut u32,
    ) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOIteratorIsValid(iterator: u32) -> u32;
  }

  #[link(name = "CoreFoundation", kind = "framework")]
  extern "C" {
    fn CFRelease(value: CfRef);
    fn CFGetTypeID(value: CfRef) -> usize;
    fn CFStringCreateWithCString(allocator: CfRef, value: *const c_char, encoding: u32) -> CfRef;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetCString(value: CfRef, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: CfRef, key: CfRef) -> CfRef;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: CfRef) -> u8;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(value: CfRef) -> isize;
    fn CFDataGetBytePtr(value: CfRef) -> *const u8;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(value: CfRef, kind: i32, output: *mut c_void) -> u8;
  }

  #[cfg(test)]
  mod tests {
    include!("volumes/tests.rs");
  }
}
