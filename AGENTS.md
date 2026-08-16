# MultiSeat Lite — Agent Instructions

## Mission

Build a free Windows multiseat application that allows two or more people to use one physical PC at the same time with separate:

* monitors
* keyboards
* mice
* audio devices where possible
* user environments/sessions

The primary home use case is:

* Seat 1: primary user on the normal Windows host
* Seat 2: child/secondary user on a second monitor with a separate keyboard and mouse

The long-term goal is functionality comparable in user experience to a simplified ASTER Multiseat, without copying ASTER branding, assets, or implementation details.

The priority is real, testable functionality and safety. Do not claim features work unless they have been implemented and verified.

---

## Product architecture: multiple seat engines

MultiSeat Lite is not tied to one implementation strategy.

The product should ultimately support at least two seat engines:

### 1. Virtual Machine backend

A secondary seat runs in a virtual machine while the primary seat remains on the Windows host.

Initial implementation:

```text
Physical Windows PC
│
├── Seat 1 — Host
│   └── Windows host session
│
└── Seat 2 — VM
    └── Windows virtual machine
```

VirtualBox is the first implemented VM backend, but the core application must not assume VirtualBox is the only possible virtualization backend.

Future VM backends may include Hyper-V or another suitable virtualization engine.

### 2. Native Windows backend

A future experimental backend should investigate ASTER-like operation using multiple Windows user/logon sessions on the same Windows installation.

Conceptually:

```text
Same Windows installation
        │
        ├── Seat 1 — Windows session A
        │   ├── Display A
        │   ├── Keyboard A
        │   └── Mouse A
        │
        └── Seat 2 — Windows session B
            ├── Display B
            ├── Keyboard B
            └── Mouse B
```

The Native Windows backend must remain separate from the VM backend and may be marked Experimental until it is proven safe and stable.

Do not patch Windows licensing/session restrictions, `termsrv.dll`, system DLLs, or undocumented Windows binaries to simulate native multi-user support.

---

## Backend selection

The UI should eventually allow a seat to choose an engine, for example:

```text
Seat engine

( ) Native Windows Session
    Best performance
    Shared Windows installation
    Experimental

(●) Virtual Machine
    Higher compatibility
    Isolated Windows environment
```

The application should detect backend capabilities and recommend a working option rather than requiring the user to understand the implementation details.

The logical seat configuration must remain independent from the selected backend.

---

## Device routing is separate from seat backend

Do not assume all devices attached to a seat must use the same routing mechanism.

A seat may use a hybrid configuration, for example:

```text
Seat backend: VirtualBox

Display:   VirtualBox presentation
Mouse:     VirtualBox USB passthrough
Keyboard:  Native MultiSeat input routing
Audio:     VirtualBox/native audio routing
```

Model routing explicitly, conceptually:

```rust
enum InputRoutingStrategy {
    VirtualBoxUsbPassthrough,
    NativeRouting,
    Disabled,
}
```

Exact names may evolve.

Never couple the persistent physical device identity to a backend-specific runtime identity.

---

## Current project status

The repository is the source of truth for the exact implementation. Inspect it before making changes.

The following major foundations have already been implemented and should not be rebuilt from scratch unless there is a demonstrated defect:

* physical display discovery
* GPU/display adapter discovery
* SetupAPI / Configuration Manager input discovery
* Raw Input discovery
* physical HID aggregation by Windows Container ID
* stable physical device identity
* ASTER/MutEnx-aware diagnostics
* interactive keyboard/mouse identification
* Seat 1 / Seat 2 configuration
* persistent seat assignments
* missing/ambiguous device reconciliation
* VirtualBox capability probing
* VirtualBox host USB parsing and correlation
* VirtualBox VM discovery
* managed Windows VM creation
* Windows 10 / Windows 11 guest profiles
* transactional VM creation and rollback
* unattended Windows installation
* installation recovery/reconciliation
* managed VM runtime states
* VirtualBox mouse USB passthrough
* VirtualBox host mouse capture disabling
* best-effort VirtualBox GUI focus protection

### Current development focus

The current priority is safe keyboard routing for the VM backend.

The real development machine demonstrated that:

* Barnen mouse USB passthrough works through VirtualBox and reaches `Captured` state.
* Barnen physical keyboard USB passthrough is not reliable.
* attempting to capture the development keyboard with VID/PID `1A2C:4C5E` has been associated with a Windows host BSOD involving the VirtualBox USB/PnP path.

Therefore:

* do not automatically attach keyboard-capable devices through VirtualBox USB passthrough unless explicitly considered safe
* do not automatically retry VirtualBox USB passthrough for development device `1A2C:4C5E`
* keep working mouse passthrough intact
* prioritize a native keyboard routing architecture for the VM backend

The next major challenge is both:

1. reading only the configured Barnen physical keyboard
2. preventing that physical keyboard from simultaneously affecting the Windows host

Raw Input solves identification/observation, not selective host suppression.

If selective suppression ultimately requires a keyboard/HID filter driver, design that boundary explicitly and safely. Do not silently introduce a kernel driver.

---

## Seat backend abstraction

Keep seat orchestration behind typed backend interfaces.

Conceptually:

```rust
trait SeatBackend {
    fn backend_kind(&self) -> BackendKind;
    fn capabilities(&self) -> Result<BackendCapabilities>;
    fn validate_seat(&self, seat: &ResolvedSeatConfig) -> Result<SeatValidation>;
    fn status(&self, seat_id: &SeatId) -> Result<SeatRuntimeStatus>;
    fn start(&self, seat: &ResolvedSeatConfig) -> Result<SeatActivationResult>;
    fn stop(&self, seat: &ResolvedSeatConfig) -> Result<SeatStopResult>;
}
```

The exact API may evolve.

Potential backend kinds include:

```text
VirtualBox
HyperV
NativeWindows
Unsupported
```

Do not place VirtualBox-specific UUIDs, VM settings, or command details inside generic `SeatConfig`.

---

## Physical hardware identity

Physical devices must use stable Windows identifiers where available.

Preferred input-device grouping:

1. Windows Container ID
2. stable PnP/device instance identity as a fallback

Do not merge devices only because they share:

* friendly name
* manufacturer
* VID/PID

Two identical keyboards may have the same model and VID/PID while still being separate physical devices.

A physical composite HID device may expose multiple logical HID collections.

The normal UI should show one assignable physical device while diagnostics may retain all member devnodes, Raw Input paths, HID collections, and USB parent information.

A physical container must not be split between different seats.

---

## VM backend principles

The VM backend is the first production-oriented path because it provides a practical route to a working second seat.

### VirtualBox

VirtualBox is currently the first implementation.

Use documented VirtualBox interfaces and `VBoxManage` operations.

Prefer direct process execution with argument arrays. Do not invoke `VBoxManage` through a shell unless there is a specific documented need.

Capture stdout, stderr, and exit status.

Backend runtime identities such as VirtualBox host USB UUIDs may change and must be resolved again when needed.

Do not persist host USB UUIDs as physical seat identity.

### Managed Windows VM

MultiSeat Lite may create and manage a Windows guest VM.

Supported managed profiles should include at least:

* Windows 10 x64
* Windows 11 x64

Windows 11-specific features such as TPM 2.0 and Secure Boot must not be imposed on Windows 10 profiles.

Do not bundle, redistribute, or generate Windows product keys.

Credentials and product keys are sensitive:

* do not log them
* do not store them in normal application configuration
* redact them in diagnostics
* persist only non-sensitive resume state

### Guest Additions

Guest Additions state must be independent from Windows installation state.

Do not report Windows as still installing merely because one Guest Additions property is absent.

Treat guest runlevel, OS evidence, Guest Additions communication, and Guest Additions version as separate evidence dimensions.

---

## VirtualBox GUI input is not a hard isolation boundary

The normal VirtualBox GUI can route host keyboard input to the guest when its window owns keyboard focus.

Existing focus protection is therefore best-effort UX, not guaranteed security/isolation.

Do not report host keyboard isolation as guaranteed merely because focus protection is active.

For a stable VM seat, physical Barnen input should use explicit seat routing and should not depend on VirtualBox GUI keyboard focus.

A future presentation model may use headless/detachable/custom presentation if that provides stronger input isolation.

---

## Native keyboard routing

The preferred VM keyboard architecture should remain separable into source and sink components.

Conceptually:

```rust
trait PhysicalKeyboardSource {
    // receives events only from the configured physical keyboard
}

trait GuestKeyboardSink {
    fn key_down(&self, event: KeyEvent) -> Result<()>;
    fn key_up(&self, event: KeyEvent) -> Result<()>;
    fn release_all(&self) -> Result<()>;
}
```

Requirements:

* Dennis keyboard must never be forwarded to Barnen
* preserve make/break semantics
* handle modifier keys correctly
* track held keys
* release all guest key state on router stop, backend failure, device disconnect, VM stop, or application exit
* never rely on VirtualBox GUI focus as the normal keyboard transport

Raw Input can be used to identify the source device, but observation is not equivalent to suppressing the same input from Windows host applications.

Investigate selective input suppression separately and honestly report if a Windows filter driver is required.

---

## Future Native Windows backend

After the VM backend is stable enough to serve as a fallback, create a separate Native Windows research/backend path.

Areas to investigate include:

* Windows session APIs / WTS APIs
* logon/session lifecycle
* interactive window stations/desktops
* physical input routing
* HID filter architecture
* Virtual HID Framework
* KMDF
* display assignment/presentation
* indirect/virtual display drivers where appropriate
* audio endpoint assignment
* GPU/session behavior

Goals:

* multiple simultaneous user environments on one Windows installation
* better resource efficiency than a full VM
* no second Windows installation for the secondary seat
* direct GPU usage where supported

Constraints:

* use documented Windows mechanisms where practical
* do not bypass Windows licensing/session restrictions
* do not patch system binaries
* do not disable security features merely to make native mode work
* mark unsupported capability as unsupported rather than faking it

The Native Windows backend may require a Windows service and one or more carefully isolated drivers. Driver work must remain outside the normal Tauri desktop process.

---

## Technology stack

Preferred application stack:

```text
Frontend:
React
TypeScript
Vite

Desktop shell:
Tauri

Native backend:
Rust

Windows-specific native code:
Rust using documented Windows APIs where practical

Additional native components:
C or C++ when required by Windows Driver Kit or APIs that are impractical from Rust
```

Do not force low-level Windows functionality into TypeScript.

React must not directly implement hardware, driver, session, or virtualization logic.

---

## Suggested repository architecture

Keep clear boundaries between generic seat logic and backend-specific implementations.

Conceptually:

```text
src/
  components/
  features/
  hooks/
  lib/
  types/

src-tauri/
  src/
    commands/
    config/
    devices/
    displays/
    audio/
    seats/
    input_routing/
    windows/
    virtualization/
      virtualbox/
      hyperv/
    native_windows/
    setup/
```

This is a guideline, not an immutable layout.

Refactor when the existing repository already has a cleaner structure.

---

## Responsibilities

### React / TypeScript

* UI
* seat/backend selection
* configuration screens
* drag and drop
* runtime status
* setup wizard
* diagnostics presentation
* user interaction

### Tauri commands

* typed bridge between frontend and native backend

### Rust

* hardware discovery
* Windows APIs
* physical device identity
* seat orchestration
* backend abstraction
* process management
* virtualization integration
* input routing
* persistence
* setup/dependency orchestration

### Native service/driver components

Only use these for functionality that genuinely cannot be implemented safely and reliably in the normal application process.

---

## Core data model

Prefer typed models rather than arbitrary JSON.

Conceptually the system includes:

```text
ApplicationConfig
SeatConfig
SeatId
SeatBackendConfig
BackendKind
PhysicalInputDevice
DisplayDevice
AudioDevice
InputRoutingStrategy
SeatRuntimeStatus
DeviceRuntimeStatus
```

Do not fabricate missing hardware information.

Backend-specific runtime data must stay separate from persistent physical identity.

---

## Safety

This project interacts with low-level Windows functionality, virtualization, USB/PnP, and potentially future drivers.

Safety has priority over convenience.

Do not automatically:

* modify Windows boot configuration
* disable driver signature enforcement
* disable Windows security features
* patch Windows binaries
* replace system DLLs
* bypass licensing/session restrictions
* modify undocumented session-management components
* install kernel drivers without explicit user consent and a clear reason
* permanently replace hardware drivers merely to make a prototype work
* alter BIOS/UEFI settings
* repeatedly retry an operation that has caused a host crash

Prefer reversible changes.

Before potentially disruptive actions:

1. detect
2. validate
3. explain
4. obtain explicit user intent where appropriate
5. perform the smallest reversible change
6. verify
7. provide recovery

### Known development safety issue

Do not automatically retry VirtualBox USB passthrough for the development keyboard identified as VID/PID `1A2C:4C5E`.

A prior real-world capture attempt was associated with a Windows `PNP_DETECTED_FATAL_ERROR` / VirtualBox USB/PnP crash path.

Mouse passthrough and keyboard passthrough must be treated independently.

---

## Reliability rules

Do not claim a feature works unless it has been implemented and, where practical, tested.

Do not replace real functionality with mock data except in explicit test/mock implementations.

Do not silently fall back to fake devices or guessed identity mappings.

When an operation fails:

* preserve the underlying error
* return structured context
* include Windows error codes where relevant
* include backend exit codes/stdout/stderr where safe
* redact credentials and product keys

Avoid `unwrap()` and `expect()` in production paths where failure is reasonably possible.

Use structured error handling.

Do not infer healthy state from a command merely returning success when stronger runtime verification is available.

---

## Runtime health model

Keep component health separate.

For example a VM seat may have:

```text
VM: Running
Display: Presented
Mouse routing: Captured
Keyboard routing: Active
Guest state: Operational
Host GUI keyboard isolation: Best effort
```

A seat must not be marked fully healthy merely because the VM process is running.

Do not collapse:

* VM state
* Windows installation state
* Guest Additions state
* input routing state
* display presentation state
* audio state

into one ambiguous boolean.

---

## Coding rules

Prefer:

* small focused modules
* strongly typed interfaces
* explicit error handling
* descriptive names
* minimal dependencies
* documented Windows/VirtualBox API wrappers
* testable business logic
* backend-neutral core models

Avoid:

* giant files
* duplicated implementations
* speculative abstractions with no current use
* unnecessary frameworks
* large rewrites when a focused change is sufficient
* introducing backend-specific assumptions into generic seat code

Before adding a dependency, determine whether the existing stack or documented Windows APIs already provide the needed capability.

---

## Windows support

This is a Windows-specific application.

Development must support the current Windows 10 x64 development host while keeping Windows 11 x64 compatibility.

Target:

```text
Windows 10 x64 — supported where technically feasible
Windows 11 x64 — supported and preferred for long-term compatibility
```

Do not spend development time on Linux or macOS unless explicitly requested.

Do not assume Windows 11 hardware requirements are satisfied on every host.

---

## Installation and first-run experience

MultiSeat Lite must ultimately be usable by a non-technical Windows user.

The final product should not require the user to manually manage backend dependencies through separate applications where automation is officially supported and legally permitted.

Desired flow:

```text
Install MultiSeat Lite
→ Run system check
→ Detect supported seat engines
→ Install/configure missing supported backend components with consent
→ Configure seats
→ Identify and assign hardware
→ Create/prepare secondary environment if required
→ Start workplaces
```

Rules:

* detect dependencies before attempting installation
* never silently install third-party software without informing the user
* request Windows elevation only when necessary
* prefer officially supported unattended/silent installers
* verify downloaded installers before execution
* do not disable Windows security features to simplify setup
* do not require normal users to manually use `VBoxManage`
* do not require manual VirtualBox USB filter creation
* do not require manual VM configuration when MultiSeat Lite can safely automate it
* preserve compatible existing installations
* never uninstall or overwrite unrelated third-party software without explicit consent
* setup operations must be resumable after reboot or failure
* BIOS/UEFI changes must be detected and explained, not silently modified

---

## UI direction

The UI should be simple and seat-oriented rather than backend-oriented.

Conceptually:

```text
UNASSIGNED

[ Monitor ]
[ Keyboard ]
[ Mouse ]

┌────────────────────────────┐
│ Dennis                     │
│ Engine: Host               │
│ Display                    │
│ Keyboard                   │
│ Mouse                      │
└────────────────────────────┘

┌────────────────────────────┐
│ Barnen                     │
│ Engine: Virtual Machine    │
│ Display                    │
│ Keyboard                   │
│ Mouse                      │
│                            │
│ [ Start Barnen ]           │
└────────────────────────────┘
```

Eventually allow the user to switch Barnen between supported engines:

```text
Virtual Machine
Native Windows Session (Experimental)
```

Show advanced backend/debug information only where useful.

Functionality and truthful status are more important than visual polish.

---

## Diagnostics and testing

Hardware-facing and backend-facing functionality should be testable independently from the UI where practical.

Useful diagnostics include:

```text
Physical device ID
Container ID
Device instance ID
USB parent
VID/PID
Friendly name
Assigned seat
Routing strategy
Backend runtime identity
Runtime state
```

Do not expose unnecessary sensitive machine information in the normal UI.

Use fake executors/interfaces for automated tests instead of attaching real USB devices or starting real VMs during ordinary unit tests.

Real device capture/start actions must be explicit development/user actions.

---

## Development workflow

Before implementing a task:

1. Read this `AGENTS.md`.
2. Inspect the existing repository and understand the current implementation.
3. Treat the repository as the source of truth for current code/state.
4. Do not assume files or APIs exist without checking.
5. Form a short implementation plan for non-trivial changes.
6. Implement the smallest complete vertical slice.
7. Run formatting and relevant checks.
8. Compile the project.
9. Run relevant tests.
10. Fix errors introduced by the change.
11. Report what was implemented, verified, and still unverified.

Do not stop after merely writing code if it can reasonably be compiled or tested locally.

Do not start the next milestone automatically when the current task asks only for investigation, diagnostics, or a bounded implementation.

Do not modify `AGENTS.md` unless the user explicitly asks to update project instructions or the task specifically requires it.

---

## Current development progression

Prefer the following high-level order unless real test results justify changing it:

```text
COMPLETED FOUNDATION
hardware discovery
→ stable physical IDs
→ HID aggregation
→ interactive identify
→ seat configuration
→ persistence
→ backend abstraction
→ VirtualBox detection
→ managed VM creation/install/recovery
→ working mouse passthrough

CURRENT
safe keyboard routing
→ selective host keyboard isolation

NEXT VM BACKEND WORK
→ dedicated second-display presentation
→ audio routing
→ lifecycle hardening
→ installer/first-run automation
→ stable VM backend release

THEN
NativeWindowsBackend research
→ native session lifecycle
→ native HID/input routing
→ native display/session presentation
→ audio/session routing
→ native backend prototype
```

Do not begin display/fullscreen work while keyboard routing remains unsafe or unverified unless the user explicitly reprioritizes it.

---

## Native backend research rules

When native development begins:

* keep it behind a separate backend/interface
* do not break the stable VM backend
* perform read-only capability research before system modification
* distinguish supported Windows behavior from hacks
* investigate licensing/session limitations explicitly
* prefer documented Windows APIs
* isolate any service/driver work in dedicated components
* make uninstall/recovery possible
* keep Native mode Experimental until verified on real hardware

The stable VM backend should remain available as a fallback even if Native mode eventually becomes the preferred option.

---

## Source of truth

The repository is the source of truth for current implementation details.

This file defines project direction and safety constraints, not exact claims about every current source file.

If repository reality conflicts with an outdated implementation detail in this document:

1. identify the conflict
2. do not silently work around it
3. preserve safety
4. explain what should be updated

For difficult Windows-specific or VirtualBox-specific problems, use current official documentation and explain architectural consequences before replacing large parts of the implementation.

The priority is a real, safe, testable multiseat program with both a stable VM path and a future native Windows path.
