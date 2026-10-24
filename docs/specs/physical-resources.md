# SPEC-PHYSICAL-RESOURCES — native worker resource keys

- Status: native component conformance on both configured Macs; public dispatch remains open in MS-LIFETIME-ENFORCE.T14.
- Surface: `volumes::resolve(&std::fs::File) -> std::io::Result<Vec<String>>`.
- Consumers: worker admission, grant validation, and recovery.
- Grounds: `DEC-LIFETIME-AUTHORITY`, section 7.
- Bindings: focused executable tests below; same-change review of `src/volumes.rs` and `src/volumes/tests.rs`.
  These are named test bindings, not a machine-enforced specification fixture.

## Contract

- **C1 — Held input and boot scope.** The input is a held regular file or directory on local APFS.
  Success returns one canonical key, thus a sorted, unique set.
  The key contains the exact native identity from `crate::launch::boot_id()`.
  Its format is `macos-nvme:<boot>:fabric:<controller-entry-id>` or
  `macos-nvme:<boot>:pcie:<PCI-endpoint-entry-id>`.
  Entry IDs contain 16 lowercase hexadecimal digits. Boot identity is nonempty, at most
  64 bytes, and contains only hexadecimal digits or hyphens. The maximum key length is 99 bytes.
  A missing boot capability refuses; there is no bootless key.

- **C2 — Physical domain.** Path aliases, APFS volumes, partitions, and namespaces on the same
  admitted physical endpoint share its key. APFS volume UUIDs and BSD names are not resource keys.
  Internal Apple Fabric storage uses its native NVMe controller identity.
  External tunneled PCIe storage uses the NVMe PCI endpoint identity, not a bridge or bus identity.

- **C3 — Complete native topology.** Each traversed provider hop has exactly one parent and a valid,
  exhausted native iterator. Missing parents, multiple parents, cycles, zero IDs, unknown classes,
  and missing or mistyped required properties refuse. Network, virtual, RAID, USB mass storage,
  multiple APFS stores, and unproved external layouts refuse.
  The admitted chain, after an optional `AppleAPFSSnapshot`, is:

  ```text
  AppleAPFSVolume → AppleAPFSContainer → AppleAPFSMedia → AppleAPFSContainerScheme
    → IOMedia [partition] → IOGUIDPartitionScheme → IOMedia [whole] → IOBlockStorageDriver
  ```

  Internal termination is `IOEmbeddedNVMeBlockDevice → IONVMeController` conformance,
  with protocol `Apple Fabric` and location `Internal`.
  External termination is `IONVMeBlockStorageDevice → IONVMeController → IOPCIDevice`,
  with protocol `PCI-Express`, location `External`, controller `IOPCITunnelCompatible=true`,
  endpoint `IOPCITunnelled=true`, and four-byte endpoint class code `02080100`.
  The APFS volume and partition have `Whole=false`; APFS synthesized and physical whole media have `Whole=true`.
  The first media's BSD major/minor match the held descriptor's `fstatfs` device identity.

- **C4 — Observation and bounds.** Two complete observations must agree on boot, mount, registry
  identities, classes, and required properties. Each observation also rereads the held mount.
  A detected change refuses. Each traversal retains at most 16 nodes and reads at most two parent
  results per hop. Native text buffers contain 128 bytes; a BSD name has at most 127 bytes.
  Native mount-source and filesystem-type fields have fixed 1024-byte and 16-byte buffers.
  Unterminated text, oversized protocol text, and a PCI class-code length other than four bytes refuse.

- **C5 — Effects and errors.** Resolution uses public native IOKit/CoreFoundation APIs and held-descriptor
  metadata. It starts no helper, opens no device, writes no target, collects no serial number,
  and changes no credential. Unsupported topology or missing properties return `Unsupported`.
  An observed mapping change returns `WouldBlock`; an invalid input type returns `InvalidInput`.
  Native operating-system failures propagate, including boot-identity `PermissionDenied`.
  Linux returns `Unsupported`; Linux runtime proof is absent.

## Executable bindings

Run on the current Mac:

```sh
CARGO_BUILD_JOBS=1 cargo test volumes:: -- --test-threads=1 --nocapture
```

Names below are in `src/volumes/tests.rs`, under `volumes::native::tests`.

| Clauses | Executable binding |
| --- | --- |
| C1, C2 | `native_system_and_data_share_one_resource`, `native_target_aliases_and_held_rename_share_keys` |
| C1, C4 | `native_boot_scope_and_resource_limits`, `changed_observations_never_return_keys` |
| C2, C3 | `physical_controller_not_apfs_volume_determines_capacity`, `external_tunneled_nvme_requires_exact_endpoint` |
| C3, C4 | `unknown_virtual_raid_and_incomplete_topologies_refuse`, `bounds_and_cycles_refuse`, `native_pci_class_code_requires_exact_type_and_length` |
| C3, C5 | `missing_native_properties_and_wrong_types_refuse`, `native_unsupported_mount_and_special_object_refuse` |

The external fixture contains selected fields from the native Mini Kytos observation.
It proves component decisions. The compiled native selections pass on both Macs.
The installed worker also resolves a kept directory and its alias on Kytos through its private
grant/result channel. File bytes survive. See [runtime evidence](../proof/WORKER-RUNTIME-T14.md).
Public dispatch, overlapping-scope reservations, and grant/commit revalidation remain open.

## Native grounds and limits

Apple's [APFS provider graph and snapshot ancestry](https://github.com/apple-oss-distributions/bless/blob/main/libbless/APFS/BLAPFSUtilities.c)
establish the APFS mapping. Its
[Disk Arbitration implementation](https://github.com/apple-oss-distributions/DiskArbitration/blob/main/diskarbitrationd/DADisk.c)
uses `f_fsid.val[0]` for BSD major/minor; `st_dev` alone is insufficient for snapshots and firmlinks.
Apple's [PCI device interface](https://github.com/apple-oss-distributions/IOPCIFamily/blob/main/IOKit/pci/IOPCIDevice.h)
and [PCI bridge implementation](https://github.com/apple-oss-distributions/IOPCIFamily/blob/main/IOPCIBridge.cpp)
establish PCI endpoint and class-code semantics. The Mini's installed `IONVMeFamily.kext`
personality binds `IONVMeController` to `IOPCIDevice` with `IOPCIClassMatch=0x01080200&0xffffff00`.
Selected native evidence is retained under `target/control-t13/kytos-native-topology.log`
and `target/control-t13/kytos-native-driver.log`.

Resolution is synchronous and belongs in the resolver worker, never the control loop.
These bounds do not impose a deadline on native calls or bound OS/library allocations.
Sampling does not exclude an unobserved change-and-revert or any change after return.
The parent retains descriptor, reservation, reboot/recovery, and grant/commit revalidation obligations.
