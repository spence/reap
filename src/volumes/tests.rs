use super::*;

fn fixture() -> Observation {
  let classes = [
    "AppleAPFSVolume",
    "AppleAPFSContainer",
    "AppleAPFSMedia",
    "AppleAPFSContainerScheme",
    "IOMedia",
    "IOGUIDPartitionScheme",
    "IOMedia",
    "IOBlockStorageDriver",
    "IOEmbeddedNVMeBlockDevice",
    "AppleANS3CGv2Controller",
  ];
  Observation {
    boot: "11111111-2222-3333-4444-555555555555".into(),
    mount: Mount {
      device: 1,
      fsid: [1, 0],
      source: b"/dev/disk3s5\0".to_vec(),
    },
    nodes: classes
      .iter()
      .enumerate()
      .map(|(i, class)| Node {
        id: (i + 1) as u64,
        class: (*class).into(),
        whole: match i {
          0 | 4 => Some(false),
          2 | 6 => Some(true),
          _ => None,
        },
        protocol: (i == 8).then(|| ("Apple Fabric".into(), "Internal".into())),
        controller: i == 9,
        tunnel_compatible: None,
        pci_class: None,
        tunnelled: None,
      })
      .collect(),
  }
}

/// The selected fields come from the read-only native Kytos observation.
fn kytos_fixture() -> Observation {
  let mut observation = fixture();
  observation.mount.source = b"/dev/disk7s1\0".to_vec();
  let ids = [
    0x100000cad,
    0x100000cac,
    0x100000ca4,
    0x100000ca3,
    0x100000ca0,
    0x100000c9b,
    0x100000c97,
    0x100000c96,
    0x100000c95,
    0x100000c90,
  ];
  for (node, id) in observation.nodes.iter_mut().zip(ids) {
    node.id = id;
  }
  observation.nodes[8].class = "IONVMeBlockStorageDevice".into();
  observation.nodes[8].protocol = Some(("PCI-Express".into(), "External".into()));
  observation.nodes[9].class = "IONVMeController".into();
  observation.nodes[9].tunnel_compatible = Some(true);
  observation.nodes.push(Node {
    id: 0x100000c7b,
    class: "IOPCIDevice".into(),
    whole: None,
    protocol: None,
    controller: false,
    tunnel_compatible: None,
    pci_class: Some([2, 8, 1, 0]),
    tunnelled: Some(true),
  });
  observation
}

#[test]
fn native_system_and_data_share_one_resource() {
  let system = File::open("/").unwrap();
  let data = File::open("/System/Volumes/Data").unwrap();
  let system_observation = observe(&system).unwrap();
  let data_observation = observe(&data).unwrap();
  assert_ne!(
    system_observation.mount.device,
    data_observation.mount.device
  );
  let system = super::super::resolve(&system).unwrap();
  let data = super::super::resolve(&data).unwrap();
  assert_eq!(system, data);
  assert_eq!(system.len(), 1);
  assert!(system[0].len() <= 128);
  println!("system/Data physical resource: {}", system[0]);
}

#[test]
fn native_target_aliases_and_held_rename_share_keys() {
  use std::os::unix::fs::symlink;
  crate::launch::boot_id().unwrap();
  let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("target/control-t13")
    .join(format!(
      "volumes-fixtures-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    ));
  std::fs::create_dir(&root).unwrap();
  let target = root.join("target");
  std::fs::create_dir(&target).unwrap();
  std::fs::write(target.join("file"), b"native resource fixture\n").unwrap();
  symlink(&target, root.join("alias")).unwrap();
  let held = File::open(&target).unwrap();
  let keys = super::super::resolve(&held).unwrap();
  for path in [
    root.join("alias"),
    root.join("alias/file"),
    target.join("file"),
  ] {
    assert_eq!(
      keys,
      super::super::resolve(&File::open(path).unwrap()).unwrap()
    );
  }
  std::fs::rename(&target, root.join("renamed")).unwrap();
  assert_eq!(keys, super::super::resolve(&held).unwrap());
  assert_eq!(
    keys,
    super::super::resolve(&File::open(root.join("renamed")).unwrap()).unwrap()
  );
  drop(held);
  std::fs::remove_file(root.join("alias")).unwrap();
  std::fs::remove_file(root.join("renamed/file")).unwrap();
  std::fs::remove_dir(root.join("renamed")).unwrap();
  std::fs::remove_dir(&root).unwrap();
  assert!(!root.exists());
}

#[test]
fn native_unsupported_mount_and_special_object_refuse() {
  let devfs = File::open("/dev").unwrap();
  assert_eq!(
    super::super::resolve(&devfs).unwrap_err().kind(),
    io::ErrorKind::Unsupported
  );
  let null = File::open("/dev/null").unwrap();
  assert_eq!(
    super::super::resolve(&null).unwrap_err().kind(),
    io::ErrorKind::InvalidInput
  );
}

#[test]
fn physical_controller_not_apfs_volume_determines_capacity() {
  let first = fixture();
  let mut second = first.clone();
  second.mount.device = 200;
  second.mount.source = b"/dev/disk4s2\0".to_vec();
  for node in &mut second.nodes[..6] {
    node.id += 200;
  }
  assert_eq!(resource(&first).unwrap(), resource(&second).unwrap());
  second.nodes.last_mut().unwrap().id += 1000;
  assert_ne!(resource(&first).unwrap(), resource(&second).unwrap());
}

#[test]
fn unknown_virtual_raid_and_incomplete_topologies_refuse() {
  for (index, class) in [
    (0, "UnknownVolume"),
    (3, "UnknownContainerScheme"),
    (4, "AppleRAIDMedia"),
    (5, "UnknownPartitionScheme"),
    (8, "IODiskImageBlockStorageDeviceInKernel"),
  ] {
    let mut observation = fixture();
    observation.nodes[index].class = class.into();
    assert!(resource(&observation).is_err(), "class: {class}");
  }
  for count in 0..fixture().nodes.len() {
    let mut observation = fixture();
    observation.nodes.truncate(count);
    assert!(resource(&observation).is_err(), "prefix: {count}");
  }
  for index in [0, 2, 4, 6] {
    let mut observation = fixture();
    observation.nodes[index].whole = None;
    assert!(resource(&observation).is_err());
  }
  let mut observation = fixture();
  observation.nodes[8].protocol = None;
  assert!(resource(&observation).is_err());
  observation.nodes[8].protocol = Some(("Virtual Interface".into(), "File".into()));
  assert!(resource(&observation).is_err());
  observation = fixture();
  observation.nodes[9].controller = false;
  assert!(resource(&observation).is_err());
}

#[test]
fn bounds_and_cycles_refuse() {
  let mut observation = fixture();
  observation
    .nodes
    .resize(MAX_NODES + 1, observation.nodes[0].clone());
  assert!(resource(&observation).is_err());
  observation = fixture();
  observation.nodes[2].id = observation.nodes[0].id;
  assert!(resource(&observation).is_err());
  observation = fixture();
  observation.nodes[2].id = 0;
  assert!(resource(&observation).is_err());
  assert!(bytes(&[b'a' as c_char; MAX_TEXT]).is_err());
}

#[test]
fn changed_observations_never_return_keys() {
  let first = fixture();
  for change in 0..5 {
    let mut second = first.clone();
    match change {
      0 => second.nodes[0].id += 100,
      1 => second.nodes[9].id += 100,
      2 => second.mount.device += 1,
      3 => second.mount.source = b"/dev/disk8s1\0".to_vec(),
      _ => second.boot = "66666666-7777-8888-9999-aaaaaaaaaaaa".into(),
    }
    let mut observations = [first.clone(), second].into_iter();
    let result = stable(|| Ok(observations.next().unwrap()));
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
  }
  let mut calls = 0;
  assert!(stable(|| {
    calls += 1;
    if calls == 2 {
      Err(unsupported("incomplete second observation"))
    } else {
      Ok(first.clone())
    }
  })
  .is_err());
  assert_eq!(calls, 2);
  assert_eq!(
    stable(|| Ok(first.clone())).unwrap(),
    vec![resource(&first).unwrap()]
  );
}

#[test]
fn missing_native_properties_and_wrong_types_refuse() {
  // SAFETY: the name is static and NUL-terminated; IOKit consumes its matching dictionary.
  let node = unsafe {
    Io::new(IOServiceGetMatchingService(
      0,
      IOBSDNameMatching(0, 0, c"disk0".as_ptr()),
    ))
    .unwrap()
  };
  assert!(property(&node, c"ReapMissingPhysicalCapability").is_err());
  assert!(number(&node, c"Whole").is_err());
  assert!(boolean(&property(&node, c"BSD Name").unwrap()).is_err());
}

#[test]
fn external_tunneled_nvme_requires_exact_endpoint() {
  let first = kytos_fixture();
  assert!(resource(&first)
    .unwrap()
    .ends_with(":pcie:0000000100000c7b"));
  let mut aliases = first.clone();
  aliases.mount.device += 100;
  aliases.mount.source = b"/dev/disk8s1\0".to_vec();
  // separate partitions, APFS volumes, and NVMe namespaces share one PCI endpoint
  for node in &mut aliases.nodes[..10] {
    node.id += 0x1000;
  }
  assert_eq!(resource(&first).unwrap(), resource(&aliases).unwrap());
  aliases.nodes[10].id += 1;
  assert_ne!(resource(&first).unwrap(), resource(&aliases).unwrap());
  for change in 0..10 {
    let mut observation = first.clone();
    match change {
      0 => observation.nodes[10].class = "IOPCI2PCIBridge".into(),
      1 => observation.nodes[10].pci_class = Some([0, 4, 6, 0]),
      2 => observation.nodes[10].pci_class = None,
      3 => observation.nodes[10].tunnelled = None,
      4 => observation.nodes[10].tunnelled = Some(false),
      5 => observation.nodes[9].tunnel_compatible = None,
      6 => observation.nodes[9].tunnel_compatible = Some(false),
      7 => observation.nodes[8].protocol = Some(("USB".into(), "External".into())),
      8 => observation.nodes[9].class = "UnknownNVMeController".into(),
      _ => {
        observation.nodes.pop();
      }
    }
    assert!(
      resource(&observation).is_err(),
      "missing external proof: {change}"
    );
  }
  let mut changed = first.clone();
  changed.nodes[10].id += 1;
  let mut observations = [first, changed].into_iter();
  assert_eq!(
    stable(|| Ok(observations.next().unwrap()))
      .unwrap_err()
      .kind(),
    io::ErrorKind::WouldBlock
  );
}

#[test]
fn native_boot_scope_and_resource_limits() {
  let first = fixture();
  let mut changed_boot = first.clone();
  changed_boot.boot = "66666666-7777-8888-9999-aaaaaaaaaaaa".into();
  assert_ne!(resource(&first).unwrap(), resource(&changed_boot).unwrap());
  for mut observation in [fixture(), kytos_fixture()] {
    observation.boot = "a".repeat(64);
    let keys = stable(|| Ok(observation.clone())).unwrap();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].len() <= 128);
    crate::launch::Resources::Bulk {
      physical_volumes: keys,
    }
    .validate()
    .unwrap();
    for invalid in [
      "".into(),
      "x:y".into(),
      "../mount-id".into(),
      "a".repeat(65),
    ] {
      observation.boot = invalid;
      assert!(resource(&observation).is_err());
    }
  }
  assert_eq!(
    stable(|| Err(io::Error::from(io::ErrorKind::PermissionDenied)))
      .unwrap_err()
      .kind(),
    io::ErrorKind::PermissionDenied
  );
}

#[test]
fn native_pci_class_code_requires_exact_type_and_length() {
  extern "C" {
    fn CFDataCreate(allocator: CfRef, bytes: *const u8, length: isize) -> CfRef;
  }
  assert!(data4(&cf_text(c"not PCI data").unwrap()).is_err());
  for length in [0, 3, 4, 5] {
    let bytes = [2, 8, 1, 0, 0];
    // SAFETY: bytes has at least length readable bytes; CFDataCreate copies those bytes.
    let data = unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), length) };
    assert!(!data.is_null());
    let data = Cf(data);
    if length == 4 {
      assert_eq!(data4(&data).unwrap(), [2, 8, 1, 0]);
    } else {
      assert!(data4(&data).is_err());
    }
  }
}
