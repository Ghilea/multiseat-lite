# MultiSeat Lite selective keyboard filter foundation

This directory is an isolated, **non-installing** foundation for selective host
keyboard suppression. It does not change the existing working path:

`Raw Input -> NativeKeyboardRouting -> VirtualBoxGuestKeyboardSink`

The C driver intercepts the documented keyboard connect request and then
unconditionally passes every packet to Kbdclass. It publishes a read-only
development interface for identity, counters, heartbeat, and an explicitly
enabled bounded event-copy stream. `EnableSuppression` is implemented only to
return `STATUS_NOT_SUPPORTED`; the callback contains no suppression state or
packet-drop branch. The INF remains a non-installable template. Nothing under
this directory is invoked by ordinary Tauri startup or the installer.

## Documented stack and chosen attachment point

For a USB keyboard, Windows' HID transport feeds HIDCLASS. HIDCLASS creates one
PDO per top-level collection (TLC); KBDHID maps the keyboard TLC's HID usages to
scan codes, and KBDCLASS queues keyboard-class input. Microsoft's
`IOCTL_INTERNAL_KEYBOARD_CONNECT` documentation explicitly describes an upper
filter saving Kbdclass' `CONNECT_DATA`, substituting its own class-service
callback, and filtering the `KEYBOARD_INPUT_DATA` transferred to the class data
queue.

The intended production attachment is therefore a **device-specific upper
filter on each selected keyboard TLC stack**, at the KBDHID/KBDCLASS callback
boundary. It is not a class-wide keyboard filter and is not a USB transport
filter. KbFiltr supplies the documented callback pattern, but its i8042/PS/2
hooks must not be copied. Microsoft's Firefly sample is useful only for the
general KMDF HID filter and controlled user-mode-interface structure.

On Windows 10 1903 and newer, an eventual signed, device-specific package should
use a per-model `DDInstall.Filters` `AddFilter` directive with
`FilterPosition=Upper`. The template intentionally contains an invalid hardware
ID token so it cannot accidentally bind globally.

Primary references:

- https://learn.microsoft.com/windows-hardware/drivers/hid/keyboard-and-mouse-hid-client-drivers
- https://learn.microsoft.com/windows-hardware/drivers/hid/hid-client-drivers
- https://learn.microsoft.com/windows-hardware/drivers/ddi/kbdmou/ni-kbdmou-ioctl_internal_keyboard_connect
- https://github.com/microsoft/Windows-driver-samples/tree/main/input/kbfiltr
- https://github.com/microsoft/Windows-driver-samples/tree/main/hid/firefly
- https://learn.microsoft.com/windows-hardware/drivers/install/inf-ddinstall-filters-section

## Raw Input consequence

Microsoft documents both ends but does not document a contract that WM_INPUT is
delivered independently after a keyboard upper filter removes packets before
the Kbdclass data queue. The documented filter point can delete data before that
queue; Raw Input is delivered by the window manager for registered keyboard
TLCs. Consequently, relying on the current Raw Input listener after deletion is
unsafe. The architectural conclusion is an inference that must be verified on
a disposable test machine, not a claimed Windows guarantee.

This milestone copies selected `KEYBOARD_INPUT_DATA` into a 256-entry (~6 KiB)
nonpaged ring only after the service has matched the exact TLC/Container ID and
explicitly enabled observation. Entries contain sequence, interrupt time,
UnitId, MakeCode, and Flags; no character history is stored. The callback never
allocates. Full rings increment `BufferOverflowCount` and discard only the
diagnostic copy, while the original packet range is still forwarded once and
unchanged. The ring is securely cleared when observation is disabled, the last
service handle closes, or PnP releases the device. A future production design
may use this copy before withholding data.
The privileged service reads that copy and implements `PhysicalKeyboardSource`;
the existing `KeyboardRouter` and `VirtualBoxGuestKeyboardSink` remain
unchanged. While suppression is off, existing Raw Input remains the identity and
diagnostic path. Ring overflow, reader loss, or malformed data immediately
cancels suppression and returns to pass-through.

## Identity and composite devices

The Rust service resolves the persistent physical identity in this order:

1. Container ID (current Barnen development value
   `F849DA33-97E4-11F1-99DC-107B448F9B30`).
2. Existing stable PnP instance fallback when Container ID is unavailable.
3. Exact present keyboard TLC instance IDs belonging to that physical device.

VID/PID `1A2C:4C5E` is diagnostic evidence only. It is never sufficient to
enable filtering. The service sends a digest of the approved physical identity
and the exact TLC instance to the corresponding per-device filter. The driver
must compare the target with its own PnP instance/container properties before a
future enable can succeed. Transient device-object pointers are never persisted.
The proposed digest is SHA-256 over the service's canonical lower-case UTF-8
physical ID; it is only an integrity/correlation field, never a substitute for
comparing the actual PnP instance and Container ID.

Only Generic Desktop/Keyboard (usage page `0x01`, usage `0x06`) TLCs are target
candidates. Consumer/media and vendor-specific TLCs remain separate logical
members of the same physical aggregate and pass through unless a future design
explicitly models and approves them. Assigning the aggregate still reserves the
whole Container ID to one seat; it does not silently broaden suppression.

## Fail-open service/driver design

The desktop UI talks to an authenticated, privileged Windows service. It never
opens the driver directly. The service owns assignment reconciliation, presence
checks, policy, diagnostics, and a renewable lease. The minimal driver owns the
callback, bounded packet-copy queue, exact target comparison, and lease expiry.

Protocol v2 defines `SetTargetDevice`, `EnableSuppression`,
`DisableSuppression`, `QueryStatus`, `Heartbeat`, `SetObservation`, and
`ReadEvents`. In this build Enable is always rejected by the kernel with
`STATUS_NOT_SUPPORTED`, regardless of target or heartbeat. SetTarget compares
the exact PnP keyboard TLC instance and Container ID with properties queried
from the filter's own PDO. Observation is rejected until that comparison
succeeds.

The user-mode policy also requires:

- the exact selected physical identity resolves once and equals Barnen;
- it differs from Dennis by Container ID/stable fallback;
- the selected keyboard TLC is present; and
- another present, non-target physical keyboard remains for the host.

Default, boot, PnP removal, service disconnect, app/service shutdown, invalid
configuration, queue overflow, and communication failure all mean pass-through.
In fact, suppression is not merely disabled by policy: no runtime state in this
binary can cause the callback to skip or alter the original Kbdclass call.

The device interface is a foundation for a future LocalSystem service, not a UI
API. Production work must still apply and verify a service-only ACL before any
sensitive deployment. Tauri must never open it directly.

## Build and test

User-mode policy (safe on any development host):

```powershell
cargo fmt --manifest-path native/keyboard-filter/service-policy/Cargo.toml -- --check
cargo test --manifest-path native/keyboard-filter/service-policy/Cargo.toml
```

Driver (build only; never install from this repository):

```powershell
./native/keyboard-filter/scripts/build-stage-a.ps1 -Configuration Debug
```

Visual Studio's C++ workload plus a matching Windows Driver Kit is required.
The pinned Stage A toolchain is Visual Studio 2022 17.x with **Desktop
development with C++**, Windows 11 SDK/WDK **10.0.26100.0**, MSVC x64 tools, and
KMDF support. Stage A enables MSVC/WDK PREfast compiler analysis and treats
warnings as errors. The binary targets Windows 10 x64 with KMDF 1.31 and the
INF's `AddFilter` minimum is Windows 10 2004 (build 19041). WDK 26100 does not
offer KMDF 1.29 as a selectable build contract; 1.31 is the nearest supported
contract. The development toolchain has WDK driver targets, KMDF headers, and
InfVerif. The complete reproducible entry point is
`scripts/build-stage-a-complete.ps1`; it runs Release/PREfast, InfVerif, Rust
tests, the driver CodeQL suites, development DVL generation, and deterministic
`stage-b/package` creation. It never installs, registers, loads, or binds the
driver. CodeQL remains an explicit prerequisite for that complete script.

`driver/MultiSeatKeyboardFilter.validation.inf` is syntax-validation only. Its
invented `HID\MSKL_VALIDATION_ONLY_00000000` ID cannot match a production
keyboard; it must never be installed or used as a Stage B binding package.

The repository-only invariant check is safe to run but is not Microsoft driver
verification:

```powershell
./native/keyboard-filter/scripts/check-pass-through-source.ps1
```

Current verification references:

- https://learn.microsoft.com/windows-hardware/drivers/devtest/static-and-dynamic-verification-tools
- https://learn.microsoft.com/windows-hardware/drivers/devtest/static-tools-and-codeql
- https://learn.microsoft.com/windows-hardware/drivers/devtest/using-static-driver-verifier-to-find-defects-in-drivers

## Test-machine checker and INF generation

The standalone Rust tool reuses the application's SetupAPI aggregation and
persisted seat configuration. Readiness is read-only:

```powershell
cargo run --manifest-path native/keyboard-filter/tools/Cargo.toml -- readiness
cargo run --manifest-path native/keyboard-filter/tools/Cargo.toml -- readiness --remote-recovery-confirmed
```

It checks Windows/build architecture, at least two physical keyboards, exact
Dennis and Barnen resolution, distinct Container IDs, keyboard TLC stacks,
unexpected existing driver service, and an explicit remote-recovery
confirmation. It prints `ReadyForPassThroughDriverTest` or `NotReady` with
reasons.

INF generation requires the exact values printed by readiness/hardware
diagnostics and never installs or overwrites a file:

```powershell
cargo run --manifest-path native/keyboard-filter/tools/Cargo.toml -- generate-inf `
  --target PHYSICAL_CONTAINER_ID `
  --container EXPECTED_CONTAINER_ID `
  --tlc 'HID\...exact instance...' `
  --hardware-id 'HID\...exact TLC hardware ID...' `
  --output C:\DriverTest\MultiSeatKeyboardFilter.inf
```

The helper verifies the target is the saved Barnen assignment, Dennis resolves
once and differs, two physical keyboards exist, the TLC belongs to Barnen, and
the hardware ID occurs on exactly one enumerated keyboard TLC. If identical
hardware produces an ambiguous INF match, generation is refused even though
the physical devices remain distinguishable by Container/instance identity.

## Stage A versus Stage B

**Stage A — build only** may run on the development machine: compile x64,
inspect the generated `.sys`/PDB and generated INF, run Rust tests, the source
invariant check, InfVerif, WDK compiler analysis, and current Microsoft driver
CodeQL suites when installed. Function role declarations remain analysis-ready,
but SDV is not present in the 24H2 WDK and is not treated as the current primary
validator. It must not stage the INF or touch PnP.

**Stage B — physical pass-through test** belongs on a disposable/separate x64
Windows machine with two real USB keyboards, remote recovery, and a manually
prepared legitimate test-signing environment. A VirtualBox guest is useful for
build tests but is not proof of physical USB-HID filter behavior.

## Signing, installation, and removal

Local kernel testing belongs on a disposable VM/test PC with an alternate input
and remote recovery. A test-signed PnP package/catalog and Windows test-signing
policy are normally required; MultiSeat Lite must not enable TESTSIGNING, disable
Secure Boot, or change BCD automatically. A production Windows 10/11 x64 package
must pass the applicable HLK/WHCP path and be submitted through Microsoft's
Hardware Developer Program for a Microsoft production signature. Current
attestation signing is for defined testing scenarios and is not a substitute for
a retail production path.

The final installer must obtain explicit elevation/consent, verify the Microsoft
signature and exact supported hardware/TLC match, stage the package with Windows
PnP APIs (or `pnputil /add-driver ... /install`), and record the published
`oemNN.inf`. Clean removal first disables and verifies fail-open, removes only
the device-specific binding/package (for example via the documented PnP APIs or
`pnputil /delete-driver oemNN.inf /uninstall`), honors a requested reboot, and
verifies that normal input returned. It must never edit keyboard class-wide
`UpperFilters` directly.

The standalone recovery script defaults to dry-run and does not require the UI:

```powershell
./native/keyboard-filter/recovery/remove-keyboard-filter.ps1 `
  -PublishedInf oemNN.inf `
  -TargetInstanceId 'HID\...'

# Only after verifying both exact values, from elevated PowerShell:
./native/keyboard-filter/recovery/remove-keyboard-filter.ps1 `
  -PublishedInf oemNN.inf `
  -TargetInstanceId 'HID\...' `
  -Execute
```

It displays the current service/package/device stack, removes only the explicit
published package with PnPUtil, rescans devices, and prints the restored stack
for verification. It never edits class-wide filters.

Signing references:

- https://learn.microsoft.com/windows-hardware/drivers/install/test-signing-driver-packages
- https://learn.microsoft.com/windows-hardware/drivers/dashboard/code-signing-attestation
- https://learn.microsoft.com/windows-hardware/drivers/install/test-signing

## Before any real suppression milestone

Use a disposable Windows test machine, remote administration, and two confirmed
physical keyboards. First install a legitimately test/production-signed
pass-through build and verify boot, PnP removal, service loss, and uninstall.
Then add only packet copying and verify its stream against Raw Input. Actual
packet withholding comes last, behind the lease and overflow fail-open tests.
Do not perform that experiment on the current development keyboard yet.

## First physical pass-through procedure (no suppression)

1. Verify both physical keyboards work before installation.
2. Run readiness with remote recovery confirmed; record both Container IDs,
   exact keyboard TLC instance/hardware IDs, and original PnP driver stacks.
3. Generate and inspect the test-machine-specific INF; run InfVerif and confirm
   its exact hardware ID occurs only once and targets the secondary TLC.
4. Manually test-sign and install only on the disposable test machine; record
   the resulting `oemNN.inf` and restart only the secondary device if required.
5. Verify Dennis/primary keyboard still works.
6. Verify the secondary keyboard still works normally in host applications.
7. Query status and verify `PacketsObserved` and `PassThroughPackets` rise
   equally, `SuppressionCapability=DisabledByBuild`, and suppression is false.
8. Stop/kill the development service and verify both keyboards still work.
9. Stop draining the observation ring until overflow increases; verify both
   keyboards still work throughout.
10. Disconnect/reconnect the secondary keyboard.
11. Verify both keyboards still work after reconnect.
12. Restart Windows with the service initially stopped.
13. Verify both keyboards still work after restart.
14. Run recovery dry-run, verify exact package/instance, execute removal, rescan
    or reboot if requested, and verify the original keyboard stack is restored.
15. Verify both keyboards one final time, retain logs/counters, and do not test
    suppression.
