# MultiSeat keyboard filter — dedicated test-machine package

DEVELOPMENT / PHYSICAL TEST ONLY. NOT CERTIFICATION EVIDENCE. The driver in
this package is pass-through-only. It cannot suppress or modify keyboard input.
Never use this package first on a machine without independent recovery access.

Nothing in this directory runs automatically. All scripts default to dry-run.
Do not use them on the primary development PC.

## Prerequisites and hard gates

1. Use a dedicated Windows 10 2004+ or Windows 11 x64 test machine.
2. Keep at least two distinct physical keyboards connected.
3. Establish an independent recovery path: remote control, a physically separate
   untargeted admin keyboard, or kernel/debug recovery from another machine.
4. Capture the pre-install identity snapshot and manually confirm both keyboards.
5. Resolve Dennis and Barnen uniquely through the persisted MultiSeat config.
6. Inspect the exact Container ID, stable physical ID, TLC instance ID, and HID
   hardware ID. Never bind by Keyboard class, friendly name, or VID/PID alone.
7. Keep the existing Raw Input → NativeKeyboardRouting → VirtualBox sink path
   available. Do not use VirtualBox USB passthrough for keyboard 1A2C:4C5E.

Readiness remains `NotReady` unless `--remote-recovery-confirmed` is supplied.
That flag is an explicit human acknowledgement; it does not detect recovery.

## Before-install snapshot

Run from a non-elevated or elevated PowerShell; it is read-only:

```powershell
.\scripts\capture-identity-snapshot.ps1 `
  -TargetInstanceId 'HID\...' `
  -ConfigPath "$env:APPDATA\se.multiseat.lite\config.json" `
  -OutputDirectory '.\snapshots\before' `
  -DennisKeyboardCheck Pass -BarnenKeyboardCheck Pass
```

The snapshot contains physical aggregates, containers, TLCs, hardware IDs,
Raw Input paths, target driver stack, UpperFilters, and relevant driver-package
listings. It never records typed keys or key contents.

## Exact INF and installation preview

```powershell
.\scripts\install-stage-b.ps1 `
  -ContainerId 'GUID' `
  -StablePhysicalId 'GUID' `
  -TlcInstanceId 'HID\...' `
  -HardwareId 'HID\VID_...&COL01' `
  -InfPath '.\target-package\MultiSeatKeyboardFilter.inf' `
  -ConfigPath "$env:APPDATA\se.multiseat.lite\config.json" `
  -RemoteRecoveryConfirmed
```

This is a dry run. The shared Rust policy rejects Dennis, ambiguity, fewer than
two physical keyboards, a TLC outside the selected container, or a hardware ID
that does not belong uniquely to that TLC. Inspect and sign the resulting exact
package before a separate future `-Execute` invocation. `-Execute` is forbidden
on the primary development machine.

## Signing strategy

### A. Local development/test path

Create a dedicated non-exported test-signing certificate on the dedicated test
environment or controlled signing workstation. Generate the catalog with
`Inf2Cat`, sign the catalog by certificate thumbprint, and verify it with
`SignTool`. `prepare-signed-package.ps1` previews these operations and only runs
them with `-Execute`. It never installs a certificate or changes kernel trust.

If the test machine needs TESTSIGNING or certificate trust changes, perform that
as a separate, explicit machine-administration procedure after reviewing Secure
Boot policy and recovery. No script in this package changes BCD, Secure Boot, or
certificate stores. A Visual Studio WDK development signature is not a trusted
retail/Microsoft signature.

### B. Production/preproduction path

Use a Hardware Dev Center submission with the appropriate EV-backed account and
choose Microsoft attestation signing or WHCP/HLK certification for the selected
Windows release. Select the matching WDK, HLK, CodeQL matrix, and signing policy
at that time. This development DVL does not claim WHCP compatibility.

Never include a private key or PFX in this package.

## Standalone recovery

Recovery depends only on PowerShell, PnPUtil, Registry query, and an independent
admin/recovery input path. It does not depend on Tauri, VirtualBox, the Rust
service, or keyboard routing:

```powershell
.\scripts\recover-stage-b.ps1 `
  -PublishedInf oem42.inf `
  -TargetInstanceId 'HID\...'
```

Inspect the dry run, then on the dedicated test machine only:

```powershell
.\scripts\recover-stage-b.ps1 `
  -PublishedInf oem42.inf `
  -TargetInstanceId 'HID\...' `
  -Execute
```

It verifies the exact package is a MultiSeat Lite keyboard package, removes only
that published INF, rescans PnP, shows the resulting stack/UpperFilters, and
verifies the published INF is gone. It never removes unrelated filters.

## Driver Verifier plan — dedicated machine only

Only after B1 is clean, from elevated Command Prompt:

```text
verifier /standard /driver MultiSeatKeyboardFilter.sys
verifier /querysettings
shutdown /r /t 0
```

The reboot activates verification. Never use `/all`. To reset from an elevated
recovery console or safe mode:

```text
verifier /reset
shutdown /r /t 0
```

Kernel dumps normally appear at `%SystemRoot%\MEMORY.DMP`; minidumps are under
`%SystemRoot%\Minidump`. Preserve the dump, matching SYS and PDB. In WinDbg use
`!analyze -v`, `lmvm MultiSeatKeyboardFilter`, inspect the failing stack/IRQL,
and record Verifier settings with `verifier /querysettings`. Do not continue to
B3 after any violation.

## Physical test phases

### B1 — load/pass-through only

Confirm Dennis and Barnen work before load. Install the exact package, then test:
both keyboards type normally; counters increase; service absent and observation
disabled still pass input; force observation-ring overflow and confirm input;
disconnect/reconnect Barnen; reboot; then cleanly uninstall and compare with the
pre-install snapshot. Any keyboard loss or stack mismatch fails B1. No suppression.

### B2 — Driver Verifier

Only after B1 succeeds, enable standard Verifier settings for
`MultiSeatKeyboardFilter.sys` only. Repeat boot, both-keyboard typing,
disconnect/reconnect, service connect/disconnect, observation, overflow, and
uninstall. Collect every dump/violation. Reset Verifier after testing. No suppression.

### B3 — packet copy/user-mode source

Only after B1 and B2 are clean, validate filter packet copies reach the privileged
service and `PhysicalKeyboardSource`, while the original packets still reach the
host unchanged. Confirm the service receives only the selected Barnen TLC and
never Dennis. This package prepares but does not implement that service path.
No suppression.

