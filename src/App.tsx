import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import type {
  ApplicationConfigSnapshot,
  AssignableDevice,
  AssignmentSlot,
  BackendProbeSnapshot,
  HardwareSnapshot,
  IdentificationStatus,
  IdentifiedInputDevice,
  KeyboardRoutingDiagnosticEvent,
  KeyboardRoutingDiagnosticStatus,
  ManagedInstallationSnapshot,
  ManagedVmRecoveryStatus,
  ManagedWindowsVersion,
  ResolvedAssignment,
  ResolvedSeat,
  SeatRuntimeSnapshot,
  SeatOperationResult,
  VirtualMachineList,
  VmCreationProposal,
} from "./types";

const slotDetails = {
  display: { label: "Display", icon: "▣" },
  keyboard: { label: "Keyboard", icon: "⌨" },
  mouse: { label: "Mouse", icon: "◉" },
} satisfies Record<Exclude<AssignmentSlot, "audioOutput">, { label: string; icon: string }>;

type VisibleSlot = keyof typeof slotDetails;

type VmCreationForm = {
  name: string;
  isoPath: string;
  memoryMb: number;
  cpus: number;
  diskGb: number;
  windowsVersion: ManagedWindowsVersion;
  installation: {
    username: string;
    password: string;
    computerName: string;
    domainName: string;
    locale: string;
    language: string;
    country: string;
    timeZone: string;
    productKey: string | null;
    installGuestAdditions: boolean;
  };
};

function App() {
  const [hardware, setHardware] = useState<HardwareSnapshot | null>(null);
  const [config, setConfig] = useState<ApplicationConfigSnapshot | null>(null);
  const [backend, setBackend] = useState<BackendProbeSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [editingSeat, setEditingSeat] = useState<string | null>(null);
  const [seatName, setSeatName] = useState("");
  const [identificationActive, setIdentificationActive] = useState(false);
  const [identificationStatus, setIdentificationStatus] = useState<IdentificationStatus | null>(null);
  const [identifiedDevice, setIdentifiedDevice] = useState<IdentifiedInputDevice | null>(null);
  const [keyboardDiagnostic, setKeyboardDiagnostic] = useState<KeyboardRoutingDiagnosticStatus | null>(null);
  const [keyboardDiagnosticEvents, setKeyboardDiagnosticEvents] = useState<KeyboardRoutingDiagnosticEvent[]>([]);
  const [virtualMachines, setVirtualMachines] = useState<VirtualMachineList>({ machines: [], warnings: [] });
  const [seatRuntime, setSeatRuntime] = useState<SeatRuntimeSnapshot | null>(null);
  const [showVmCreation, setShowVmCreation] = useState(false);
  const [creationProposal, setCreationProposal] = useState<VmCreationProposal | null>(null);
  const [vmForm, setVmForm] = useState<VmCreationForm>({
    name: "Windows - Barnen", isoPath: "", memoryMb: 4096, cpus: 2, diskGb: 72,
    windowsVersion: "windows10",
    installation: {
      username: "Barnen", password: "", computerName: "BARNEN-PC", domainName: "multiseat.local",
      locale: "sv_SE", language: "sv", country: "SE", timeZone: "",
      productKey: null, installGuestAdditions: true,
    },
  });
  const [managedInstallation, setManagedInstallation] = useState<ManagedInstallationSnapshot | null>(null);
  const [recoveryStatus, setRecoveryStatus] = useState<ManagedVmRecoveryStatus | null>(null);
  const [showResumeInstallation, setShowResumeInstallation] = useState(false);
  const highlightTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = async () => {
    setError(null);
    try {
      const [hardwareSnapshot, configuration, backendSnapshot] = await Promise.all([
        invoke<HardwareSnapshot>("get_hardware_snapshot"),
        invoke<ApplicationConfigSnapshot>("get_application_config"),
        invoke<BackendProbeSnapshot>("get_backend_probe").catch(() => null),
      ]);
      setHardware(hardwareSnapshot);
      setConfig(configuration);
      setBackend(backendSnapshot);
      const barnen = configuration.configuration.seats[1];
      if (backendSnapshot?.virtualBox.operational) {
        const [machines, runtime] = await Promise.all([
          invoke<VirtualMachineList>("list_virtual_box_vms"),
          barnen ? invoke<SeatRuntimeSnapshot>("get_seat_runtime_status", { seatId: barnen.id }) : Promise.resolve(null),
        ]);
        setVirtualMachines(machines);
        setSeatRuntime(runtime);
      }
    } catch (reason) {
      setError(String(reason));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  useEffect(() => {
    const unlisten = listen<KeyboardRoutingDiagnosticEvent>("keyboard-routing-diagnostic", (event) => {
      setKeyboardDiagnosticEvents((current) => [...current.slice(-11), event.payload]);
      if (event.payload.action === "backendError") {
        setError(`Keyboard routing stopped: ${event.payload.detail}`);
        void invoke<KeyboardRoutingDiagnosticStatus>("stop_keyboard_routing_diagnostic")
          .then(setKeyboardDiagnostic)
          .catch((reason) => setError(`Keyboard routing failed and cleanup reported: ${String(reason)}`));
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
      void invoke("stop_keyboard_routing_diagnostic");
    };
  }, []);

  useEffect(() => {
    const barnen = config?.configuration.seats[1];
    const backendSeat = barnen
      ? config.configuration.backends.virtualBox.seats[barnen.id]
      : undefined;
    if (!barnen || !backendSeat?.managedByMultiseat || !backendSeat.installation) {
      setManagedInstallation(null);
      return;
    }
    let disposed = false;
    let polling = false;
    let timer: number | null = null;
    const update = async () => {
      if (polling) return;
      polling = true;
      try {
        const snapshot = await invoke<ManagedInstallationSnapshot>("get_managed_vm_installation_status", { seatId: barnen.id });
        if (!disposed) {
          setManagedInstallation(snapshot);
          if (snapshot.installation.state === "failed") {
            const recovery = await invoke<ManagedVmRecoveryStatus>("get_managed_vm_recovery_status", { seatId: barnen.id });
            if (!disposed) setRecoveryStatus(recovery);
          } else {
            setRecoveryStatus(null);
          }
          if (snapshot.monitoringComplete && timer !== null) {
            window.clearInterval(timer);
            timer = null;
          }
        }
      } catch (reason) {
        if (!disposed) setError(String(reason));
      } finally {
        polling = false;
      }
    };
    void update();
    if (["ready", "failed"].includes(backendSeat.installation.state)) return () => { disposed = true; };
    timer = window.setInterval(() => void update(), 5000);
    return () => { disposed = true; if (timer !== null) window.clearInterval(timer); };
  }, [config]);

  useEffect(() => {
    const unlisten = listen<IdentifiedInputDevice>("input-device-identified", (event) => {
      setIdentifiedDevice(event.payload);
      if (highlightTimer.current) clearTimeout(highlightTimer.current);
      highlightTimer.current = setTimeout(() => setIdentifiedDevice(null), 2700);
    });
    return () => {
      void unlisten.then((dispose) => dispose());
      void invoke("stop_input_identification");
      if (highlightTimer.current) clearTimeout(highlightTimer.current);
    };
  }, []);

  const unassigned = useMemo(() => {
    if (!hardware || !config) return [];
    const assigned = {
      display: new Set(config.configuration.seats.flatMap((seat) => seat.devices.display ?? [])),
    };
    const assignedInputs = new Set(config.resolvedSeats.flatMap((seat) => [
      seat.devices.keyboard?.id,
      seat.devices.mouse?.id,
    ].filter((id): id is string => Boolean(id))));
    const displays: AssignableDevice[] = hardware.pnpMonitors
      .filter((monitor) => !assigned.display.has(monitor.instanceId))
      .map((monitor) => ({
        id: monitor.instanceId,
        containerId: null,
        slot: "display",
        capabilities: null,
        name: monitorName(monitor.instanceId, monitor.friendlyName, hardware),
        secondary: monitor.hardwareIds[0] ?? monitor.instanceId,
      }));
    const inputs: AssignableDevice[] = hardware.inputDevices.flatMap((device) => {
      if (assignedInputs.has(device.id)) return [];
      const slot = device.capabilities.keyboard ? "keyboard" : "mouse";
      const capabilityLabel = device.capabilities.keyboard && device.capabilities.mouse
        ? "Keyboard + mouse"
        : device.capabilities.keyboard ? "Keyboard" : "Mouse";
      return [{
        id: device.id,
        containerId: device.containerId,
        slot,
        capabilities: device.capabilities,
        name: device.friendlyName ?? capabilityLabel,
        secondary:
          device.vendorId && device.productId
            ? `${capabilityLabel} · VID ${device.vendorId} · PID ${device.productId}`
            : `${capabilityLabel} · ${device.id}`,
      }];
    });
    return [...displays, ...inputs];
  }, [hardware, config]);

  const mutate = async (operation: () => Promise<ApplicationConfigSnapshot>) => {
    setBusy(true);
    setError(null);
    try {
      setConfig(await operation());
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const assign = (seatId: string, device: AssignableDevice) =>
    mutate(() =>
      invoke<ApplicationConfigSnapshot>("assign_device", {
        seatId,
        slot: device.slot,
        deviceId: device.id,
      }),
    );

  const unassign = (seatId: string, slot: VisibleSlot) =>
    mutate(() =>
      invoke<ApplicationConfigSnapshot>("unassign_device", { seatId, slot }),
    );

  const saveSeatName = (seatId: string) => {
    if (!seatName.trim()) return;
    void mutate(() =>
      invoke<ApplicationConfigSnapshot>("update_seat", {
        seatId,
        name: seatName,
      }),
    ).then(() => setEditingSeat(null));
  };

  const toggleIdentification = async () => {
    setBusy(true);
    setError(null);
    try {
      if (identificationActive) {
        const status = await invoke<IdentificationStatus>("stop_input_identification");
        setIdentificationStatus(status);
        setIdentificationActive(false);
        setIdentifiedDevice(null);
      } else {
        const status = await invoke<IdentificationStatus>("start_input_identification");
        setIdentificationStatus(status);
        setIdentificationActive(status.active);
      }
    } catch (reason) {
      setError(String(reason));
      setIdentificationActive(false);
    } finally {
      setBusy(false);
    }
  };

  const toggleKeyboardDiagnostic = async (seatId: string) => {
    setBusy(true);
    setError(null);
    try {
      if (keyboardDiagnostic?.active) {
        setKeyboardDiagnostic(await invoke<KeyboardRoutingDiagnosticStatus>("stop_keyboard_routing_diagnostic"));
      } else {
        setKeyboardDiagnosticEvents([]);
        setKeyboardDiagnostic(await invoke<KeyboardRoutingDiagnosticStatus>("start_keyboard_routing_diagnostic", { seatId }));
      }
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const startKeyboardInjectionTest = async (seatId: string) => {
    if (!window.confirm(
      "Start the native keyboard injection prototype? Only Barnen's configured physical keyboard will be forwarded to the running managed VM. Windows host suppression is NOT implemented, so the same keys may also affect the host.",
    )) return;
    setBusy(true);
    setError(null);
    try {
      setKeyboardDiagnosticEvents([]);
      setKeyboardDiagnostic(await invoke<KeyboardRoutingDiagnosticStatus>("start_native_keyboard_injection_test", { seatId }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const selectVm = async (seatId: string, vmId: string) => {
    await mutate(() => invoke<ApplicationConfigSnapshot>("select_virtual_box_vm", { seatId, vmId }));
    setSeatRuntime(await invoke<SeatRuntimeSnapshot>("get_seat_runtime_status", { seatId }));
  };

  const runSeatAction = async (
    command: "start_virtual_box_seat" | "stop_virtual_box_seat" | "release_virtual_box_seat_devices" | "allow_virtual_box_host_input_temporarily" | "lock_virtual_box_seat_input",
    seatId: string,
  ) => {
    if (command === "start_virtual_box_seat" && !window.confirm(
      "Start Barnen? The configured mouse will be captured by the VM. Keyboard USB passthrough remains disabled; native keyboard routing is only a diagnostic prototype.",
    )) return;
    if (command === "allow_virtual_box_host_input_temporarily" && !window.confirm(
      "Temporarily allow Dennis's host keyboard and mouse to interact with the VirtualBox window? Barnen's USB devices remain attached to the guest.",
    )) return;
    setBusy(true);
    setError(null);
    try {
      const result = await invoke<SeatOperationResult>(command, { seatId });
      setSeatRuntime(result.runtime);
      if (!result.success) {
        setError(`${result.errors.join("; ")}${result.rollbackAttempted ? " Rollback was attempted." : ""}`);
      }
      setBackend(await invoke<BackendProbeSnapshot>("get_backend_probe"));
    } catch (reason) {
      setError(String(reason));
      setSeatRuntime(await invoke<SeatRuntimeSnapshot>("get_seat_runtime_status", { seatId }).catch(() => null));
    } finally {
      setBusy(false);
    }
  };

  const openVmCreation = async () => {
    try {
      const proposal = await invoke<VmCreationProposal>("get_virtual_box_vm_creation_proposal");
      setCreationProposal(proposal);
      setVmForm((current) => ({
        ...current,
        memoryMb: proposal.memoryMb,
        cpus: proposal.cpus,
        diskGb: proposal.diskGb,
        windowsVersion: proposal.defaultWindowsVersion,
        installation: {
          ...current.installation,
          username: proposal.installationDefaults.username,
          computerName: proposal.installationDefaults.computerName,
          domainName: proposal.installationDefaults.domainName,
          locale: proposal.installationDefaults.locale,
          language: proposal.installationDefaults.language,
          country: proposal.installationDefaults.country,
          timeZone: proposal.installationDefaults.timeZone,
          installGuestAdditions: proposal.installationDefaults.installGuestAdditions,
          password: "",
          productKey: null,
        },
      }));
      setShowVmCreation(true);
    } catch (reason) {
      setError(String(reason));
    }
  };

  const selectWindowsIso = async () => {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Windows ISO", extensions: ["iso"] }],
    });
    if (selected) setVmForm((current) => ({ ...current, isoPath: selected }));
  };

  const createVm = async (seatId: string) => {
    if (!creationProposal || !window.confirm(
      `Create and register “${vmForm.name}” with ${vmForm.memoryMb} MB RAM, ${vmForm.cpus} CPUs and a ${vmForm.diskGb} GB disk?`,
    )) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("create_virtual_box_vm", { seatId, request: vmForm });
      setShowVmCreation(false);
      await refresh();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setVmForm((current) => ({
        ...current,
        installation: { ...current.installation, password: "", productKey: null },
      }));
      setBusy(false);
    }
  };

  const resumeManagedInstallation = async (seatId: string) => {
    if (recoveryStatus?.credentialsRequired && !vmForm.installation.password) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("resume_managed_vm_installation", {
        seatId,
        installation: vmForm.installation,
      });
      setShowResumeInstallation(false);
      setRecoveryStatus(null);
      await refresh();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setVmForm((current) => ({
        ...current,
        installation: { ...current.installation, password: "", productKey: null },
      }));
      setBusy(false);
    }
  };

  if (!hardware || !config) {
    return (
      <main className="loading-screen">
        <div className="brand-mark" aria-hidden="true"><span /><span /><span /></div>
        <p>{error ?? "Discovering physical hardware…"}</p>
        {error && <button onClick={() => void refresh()}>Try again</button>}
      </main>
    );
  }

  return (
    <main className="shell">
      <header className="topbar">
        <div className="brand-mark" aria-hidden="true"><span /><span /><span /></div>
        <div>
          <p className="eyebrow">Seat configuration</p>
          <h1>Multiseat <strong>Lite</strong></h1>
        </div>
        <div className="system-context">
          <span className={hasAsterRuntimeEvidence(hardware.environment.aster) ? "dot amber" : "dot"} />
          Session {hardware.environment.currentSessionId}
          {hardware.environment.aster.installationDetected && " · ASTER installed"}
          {hasAsterRuntimeEvidence(hardware.environment.aster) && " · runtime components detected"}
        </div>
      </header>

      <section className="page-heading">
        <div>
          <p className="eyebrow">Milestone 2</p>
          <h2>Who gets what?</h2>
          <p>Assign physical PnP hardware. Changes are validated and saved automatically.</p>
        </div>
        <div className="heading-actions">
          <button
            className={`identify-button ${identificationActive ? "active" : ""}`}
            disabled={busy}
            onClick={() => void toggleIdentification()}
          >
            {identificationActive ? "Stop identifying" : "Identify keyboard/mouse"}
          </button>
          <button className="secondary-button" disabled={busy} onClick={() => void refresh()}>
            ↻ Refresh hardware
          </button>
        </div>
      </section>

      {error && <div className="notice error-notice">{error}</div>}
      {config.loadWarning && <div className="notice warning-notice">{config.loadWarning}</div>}

      <section className="backend-panel">
        <div>
          <p className="eyebrow">System / Backend</p>
          <h3>Virtualization capability</h3>
          <small>
            {backend
              ? `${backend.system.productName ?? "Windows"} · build ${backend.system.build ?? "unknown"}`
              : "Backend probe unavailable"}
          </small>
        </div>
        <BackendFact
          label="VirtualBox"
          value={backend?.virtualBox.installed
            ? `${backend.virtualBox.version ?? "Installed"} · USB ${backend.virtualBox.operational ? "available" : "unavailable"}`
            : "Not installed"}
        />
        <BackendFact
          label="Hyper-V"
          value={backend
            ? `${backend.hyperV.featureState} · hypervisor ${backend.hyperV.hypervisorActive ? "present" : "not present"}`
            : "Unknown"}
        />
        <BackendFact label="Experimental backend" value={backend?.selectedBackend ?? "None"} />
      </section>

      {identificationActive && (
        <section className="identification-panel" aria-live="polite">
          <div className="identification-radar"><span /></div>
          <div>
            <p className="eyebrow">Identification mode active</p>
            <h3>Move a mouse or press a key to identify it.</h3>
            {hasAsterRuntimeEvidence(hardware.environment.aster) && (
              <p>
                ASTER input components are running or visible in this session. Workplace state is
                {` ${hardware.environment.aster.workplaceState}`}; only session-visible input may be identifiable.
              </p>
            )}
            {identificationStatus?.correlatedPhysicalDevices === 0 && (
              <p>No session-visible keyboard/mouse currently correlates directly with a physical PnP device.</p>
            )}
          </div>
          {identifiedDevice && (
            <div className="identified-summary">
              <span>This device</span>
              <strong>{identifiedDevice.friendlyName ?? identifiedDevice.deviceType}</strong>
              <small>
                {identifiedDevice.vendorId && identifiedDevice.productId
                  ? `VID ${identifiedDevice.vendorId} · PID ${identifiedDevice.productId}`
                  : identifiedDevice.stablePhysicalDeviceId}
              </small>
            </div>
          )}
        </section>
      )}

      <section className="unassigned-section">
        <div className="section-title">
          <div>
            <p className="eyebrow">Available hardware</p>
            <h3>Unassigned devices <span>{unassigned.length}</span></h3>
          </div>
          <p>Physical PnP devices only · ASTER proxies excluded</p>
        </div>
        <div className="device-list">
          {unassigned.length === 0 && <p className="empty-message">All discovered devices are assigned.</p>}
          {unassigned.map((device) => (
            <article
              className={`device-card ${isIdentified(device.id, device.containerId, identifiedDevice) ? "identified" : ""}`}
              key={`${device.slot}:${device.id}`}
            >
              <div className={`device-icon ${device.slot}`}>{slotDetails[device.slot].icon}</div>
              <div className="device-copy">
                <strong>{device.name}</strong>
                <span>{device.secondary}</span>
                {device.capabilities?.keyboard && device.capabilities.mouse && (
                  <small>Assigning reserves both keyboard and mouse functions for the same seat.</small>
                )}
              </div>
              <div className="assign-actions">
                {isIdentified(device.id, device.containerId, identifiedDevice) && (
                  <span className="this-device-badge">This device</span>
                )}
                {config.resolvedSeats.map((seat) => (
                  <button disabled={busy} key={seat.id} onClick={() => void assign(seat.id, device)}>
                    Assign to {seat.name}
                  </button>
                ))}
              </div>
            </article>
          ))}
        </div>
      </section>

      <section className="seat-grid">
        {config.resolvedSeats.map((seat, index) => (
          <article className="seat-panel" key={seat.id}>
            <header>
              <div className="seat-number">0{index + 1}</div>
              <div>
                <p className="eyebrow">Seat {index + 1}</p>
                {editingSeat === seat.id ? (
                  <div className="name-editor">
                    <input value={seatName} autoFocus onChange={(event) => setSeatName(event.target.value)} />
                    <button onClick={() => saveSeatName(seat.id)}>Save</button>
                  </div>
                ) : (
                  <h3>{seat.name}</h3>
                )}
              </div>
              {editingSeat !== seat.id && (
                <button
                  className="edit-button"
                  aria-label={`Rename ${seat.name}`}
                  onClick={() => { setEditingSeat(seat.id); setSeatName(seat.name); }}
                >
                  Rename
                </button>
              )}
            </header>
            <div className="slots">
              {(Object.keys(slotDetails) as VisibleSlot[]).map((slot) => (
                <SeatSlot
                  key={slot}
                  slot={slot}
                  assignment={seat.devices[slot]}
                  identifiedDevice={identifiedDevice}
                  usbMapping={seat.devices[slot] && slot !== "display"
                    ? backend?.usbCorrelations.find((mapping) =>
                        mapping.physicalDeviceId.toLowerCase() === seat.devices[slot]?.id.toLowerCase()) ?? null
                    : null}
                  disabled={busy}
                  onUnassign={() => void unassign(seat.id, slot)}
                />
              ))}
            </div>
            {index === 1 && (
              <VirtualBoxSeatRuntime
                seat={seat}
                selectedVmId={config.configuration.backends.virtualBox.seats[seat.id]?.vmId ?? ""}
                machines={virtualMachines}
                runtime={seatRuntime}
                keyboardDiagnostic={keyboardDiagnostic}
                keyboardDiagnosticEvents={keyboardDiagnosticEvents}
                installation={managedInstallation}
                recovery={recoveryStatus}
                showResume={showResumeInstallation}
                backendOperational={Boolean(backend?.virtualBox.operational)}
                busy={busy}
                showCreation={showVmCreation}
                proposal={creationProposal}
                form={vmForm}
                onFormChange={setVmForm}
                onSelectVm={(vmId) => void selectVm(seat.id, vmId)}
                onOpenCreation={() => void openVmCreation()}
                onCancelCreation={() => {
                  setShowVmCreation(false);
                  setVmForm((current) => ({ ...current, installation: { ...current.installation, password: "", productKey: null } }));
                }}
                onCreate={() => void createVm(seat.id)}
                onOpenResume={() => {
                  if (recoveryStatus) {
                    setVmForm((current) => ({
                      ...current,
                      installation: {
                        ...current.installation,
                        computerName: recoveryStatus.suggestedComputerName,
                        domainName: recoveryStatus.suggestedDomainName,
                      },
                    }));
                  }
                  setShowResumeInstallation(true);
                }}
                onCancelResume={() => {
                  setShowResumeInstallation(false);
                  setVmForm((current) => ({ ...current, installation: { ...current.installation, password: "", productKey: null } }));
                }}
                onResume={() => void resumeManagedInstallation(seat.id)}
                onSelectIso={() => void selectWindowsIso()}
                onStart={() => void runSeatAction("start_virtual_box_seat", seat.id)}
                onStop={() => void runSeatAction("stop_virtual_box_seat", seat.id)}
                onRelease={() => void runSeatAction("release_virtual_box_seat_devices", seat.id)}
                onAllowHostInput={() => void runSeatAction("allow_virtual_box_host_input_temporarily", seat.id)}
                onLockSeatInput={() => void runSeatAction("lock_virtual_box_seat_input", seat.id)}
                onToggleKeyboardDiagnostic={() => void toggleKeyboardDiagnostic(seat.id)}
                onStartKeyboardInjection={() => void startKeyboardInjectionTest(seat.id)}
              />
            )}
          </article>
        ))}
      </section>

      <footer>
        <span>Saved automatically</span>
        <code title={config.configurationPath}>{config.configurationPath}</code>
      </footer>
    </main>
  );
}

function VirtualBoxSeatRuntime({
  seat, selectedVmId, machines, runtime, keyboardDiagnostic, keyboardDiagnosticEvents, installation, recovery, backendOperational, busy, showCreation, showResume, proposal, form,
  onFormChange, onSelectVm, onOpenCreation, onCancelCreation, onCreate, onOpenResume, onCancelResume, onResume, onSelectIso, onStart, onStop, onRelease, onAllowHostInput, onLockSeatInput,
  onToggleKeyboardDiagnostic,
  onStartKeyboardInjection,
}: {
  seat: ResolvedSeat;
  selectedVmId: string;
  machines: VirtualMachineList;
  runtime: SeatRuntimeSnapshot | null;
  keyboardDiagnostic: KeyboardRoutingDiagnosticStatus | null;
  keyboardDiagnosticEvents: KeyboardRoutingDiagnosticEvent[];
  installation: ManagedInstallationSnapshot | null;
  recovery: ManagedVmRecoveryStatus | null;
  backendOperational: boolean;
  busy: boolean;
  showCreation: boolean;
  showResume: boolean;
  proposal: VmCreationProposal | null;
  form: VmCreationForm;
  onFormChange: Dispatch<SetStateAction<typeof form>>;
  onSelectVm: (vmId: string) => void;
  onOpenCreation: () => void;
  onCancelCreation: () => void;
  onCreate: () => void;
  onOpenResume: () => void;
  onCancelResume: () => void;
  onResume: () => void;
  onSelectIso: () => void;
  onStart: () => void;
  onStop: () => void;
  onRelease: () => void;
  onAllowHostInput: () => void;
  onLockSeatInput: () => void;
  onToggleKeyboardDiagnostic: () => void;
  onStartKeyboardInjection: () => void;
}) {
  const running = runtime?.status === "running" || runtime?.status === "partiallyRunning";
  const selectedProfile = proposal?.profiles.find((profile) => profile.windowsVersion === form.windowsVersion);
  const fullHostname = form.installation.computerName.trim() && form.installation.domainName.trim()
    ? `${form.installation.computerName.trim().toLowerCase()}.${form.installation.domainName.trim().toLowerCase()}`
    : "Incomplete";
  return (
    <section className="runtime-panel">
      <div className="runtime-heading">
        <div><p className="eyebrow">VirtualBox second seat</p><h4>Barnen runtime</h4></div>
        <span className={`runtime-state ${runtime?.status ?? "notConfigured"}`}>{runtime?.status ?? "Not configured"}</span>
      </div>
      {!backendOperational ? <p className="runtime-message">VirtualBox is not operational.</p> : (
        <>
          <label className="vm-selector">
            <span>Virtual machine</span>
            <select disabled={busy || running} value={selectedVmId} onChange={(event) => onSelectVm(event.target.value)}>
              <option value="">Select an existing Windows VM…</option>
              {machines.machines.map((vm) => <option value={vm.uuid} key={vm.uuid}>{vm.name} · {vm.state} · {vm.guestOsDescription ?? "unknown OS"}</option>)}
            </select>
          </label>
          {machines.warnings.map((warning) => <small className="runtime-warning" key={warning}>{warning}</small>)}
          <div className="runtime-components">
            <RuntimeComponent label="VM" value={runtime?.vmState ?? "Not selected"} healthy={runtime?.vmState === "running"} />
            <RuntimeComponent label="Keyboard" value={runtime ? routingStatusLabel(runtime.keyboard) : seat.devices.keyboard ? "Configured" : "Not configured"} healthy={runtime?.keyboard.routingStatus === "active"} />
            <RuntimeComponent label="Mouse" value={runtime ? routingStatusLabel(runtime.mouse) : seat.devices.mouse ? "Configured" : "Not configured"} healthy={runtime?.mouse.routingStatus === "active"} />
          </div>
          {runtime && (
            <div className="input-isolation-summary" aria-live="polite">
              <strong>Managed-seat input isolation</strong>
              <div className="security-summary">
                <span>Keyboard routing: {routingStrategyLabel(runtime.keyboard.routingStrategy)} / {routingStatusLabel(runtime.keyboard)}</span>
                <span>Keyboard USB safety: {safetyLabel(runtime.keyboard.usbPassthroughSafety)}</span>
                <span>Mouse routing: {routingStrategyLabel(runtime.mouse.routingStrategy)} / {routingStatusLabel(runtime.mouse)}</span>
                <span>Mouse USB safety: {safetyLabel(runtime.mouse.usbPassthroughSafety)}</span>
                <span>Dennis keyboard USB: {runtime.inputIsolation.dennisKeyboardAttached ? "Unexpectedly attached" : "Not attached"}</span>
                <span>Dennis mouse USB: {runtime.inputIsolation.dennisMouseAttached ? "Unexpectedly attached" : "Not attached"}</span>
                <span>Host mouse capture: {runtime.inputIsolation.mouseCaptureDisabled ? "Disabled" : runtime.inputIsolation.mouseCapturePolicy ?? "Unknown"}</span>
                <span>Mouse Integration: {runtime.inputIsolation.mouseIntegration.observed === "unknown" ? `Unknown (${runtime.inputIsolation.mouseIntegration.control === "manualRequired" ? "manual action required" : "not verified"})` : runtime.inputIsolation.mouseIntegration.observed}</span>
                <span>Mouse isolation: {runtime.inputIsolation.mouseIsolation}</span>
                <span>Focus protection: {runtime.inputIsolation.focusProtectionActive ? "Active" : "Inactive"}</span>
                <span>Host GUI keyboard isolation: Best effort</span>
              </div>
              <div className="usb-diagnostics">
                <UsbRuntimeDiagnostic label="Keyboard" component={runtime.keyboard} />
                <UsbRuntimeDiagnostic label="Mouse" component={runtime.mouse} />
              </div>
              {runtime.inputIsolation.mouseCapturePolicyError && <small className="runtime-warning">{runtime.inputIsolation.mouseCapturePolicyError}</small>}
              {runtime.inputIsolation.mouseIntegration.message && <small className="runtime-warning">{runtime.inputIsolation.mouseIntegration.message}</small>}
              {runtime.inputIsolation.focusProtectionError && <small className="runtime-warning">{runtime.inputIsolation.focusProtectionError}</small>}
              {runtime.keyboard.safetyReason && <details><summary>Keyboard backend safety details</summary><p>{runtime.keyboard.safetyReason}</p></details>}
            </div>
          )}
          {runtime?.keyboard.routingStrategy === "nativeKeyboardRouting" && (
            <div className="installation-progress" aria-live="polite">
              <strong>Native keyboard routing prototype</strong>
              <p>Listens only to the configured Barnen keyboard through Raw Input. Diagnostic mode uses a mock sink; the explicit injection test targets the managed VM without USB-attaching the keyboard.</p>
              <div className="security-summary">
                <span>Source: PhysicalKeyboardSource / Raw Input</span>
                <span>Guest sink: {keyboardDiagnostic?.transport === "virtualBoxScancodePrototype" ? "VirtualBoxGuestKeyboardSink" : "Diagnostic only"}</span>
                <span>Keyboard guest routing: {keyboardDiagnostic?.active && keyboardDiagnostic.realGuestInjectionEnabled ? "Active" : "Inactive"}</span>
                <span>Host keyboard suppression: Not implemented</span>
                <span>VirtualBox GUI keyboard isolation: Best effort</span>
                <span>Session: {keyboardDiagnostic?.currentSessionId ?? "Not active"}</span>
              </div>
              <div className="runtime-actions">
                {keyboardDiagnostic?.active ? (
                  <button className="secondary-button" disabled={busy} onClick={onToggleKeyboardDiagnostic}>Stop native keyboard routing</button>
                ) : (
                  <>
                    <button className="secondary-button" disabled={busy} onClick={onToggleKeyboardDiagnostic}>Start keyboard diagnostic</button>
                    <button className="primary-action" disabled={busy || runtime.vmState !== "running"} onClick={onStartKeyboardInjection}>Start native keyboard injection test</button>
                  </>
                )}
              </div>
              {keyboardDiagnosticEvents.length > 0 && <details open><summary>Recent normalized events</summary>
                {keyboardDiagnosticEvents.map((event, index) => <p key={`${event.action}-${index}`}>
                  {event.action}{event.event ? ` / scan 0x${event.event.scanCode.toString(16).toUpperCase()} / ${event.event.transition}${event.event.modifier ? ` / ${event.event.modifier}` : ""}` : ""}
                </p>)}
              </details>}
              <small>This diagnostic does not make the keyboard exclusive. Windows may still receive the same keystrokes.</small>
            </div>
          )}
          {installation && (
            <div className="installation-progress" aria-live="polite">
              <div><span>Managed VM status</span><strong>{managedStatusLabel(installation.installation.observed.managed)}</strong></div>
              <div className="progress-track"><span style={{ width: `${installationPercent(installation.installation.state)}%` }} /></div>
              <ol className="install-steps">
                {["Virtual machine created", "Windows installation prepared", "Installing Windows", "Installing Guest Additions", "Final verification"].map((label, index) => (
                  <li className={index < installationStage(installation.installation.state) ? "done" : index === installationStage(installation.installation.state) ? "active" : ""} key={label}>{label}</li>
                ))}
              </ol>
              <div className="security-summary">
                <span>Windows: {windowsStatusLabel(installation.installation.observed.windows)}</span>
                <span>Guest Additions: {guestAdditionsStatusLabel(installation.installation.observed.guestAdditions)}</span>
                <span>Version: {installation.installation.observed.evidence.guestAdditionsVersion ?? "Unknown"}</span>
                <span>Guest runlevel: {installation.installation.observed.runLevel}</span>
                <span>VM: {installation.vmState}</span>
              </div>
              <small>{installation.installation.windowsVersion === "windows10" ? "Windows 10 x64" : "Windows 11 x64"}</small>
              <details><summary>Reconciliation evidence</summary>
                {installation.installation.observed.evidence.runlevelProbes.map((probe) => (
                  <p key={probe.level}>{probe.level}: exit {probe.exitCode ?? "unavailable"} · {probe.elapsedMs} ms · {probe.success ? "reached" : "not reached"}</p>
                ))}
                <p>Guest OS product: {installation.installation.observed.evidence.guestOsProduct ?? "Unknown"}<br />Guest Additions host version checked: {installation.installation.observed.evidence.guestAddHostVersionLastChecked ?? "Unknown"}<br />Reset counter: {installation.installation.observed.evidence.resetCounter ?? "Unknown"}</p>
                {installation.installation.observed.evidence.reconciliationEvent && <p>{installation.installation.observed.evidence.reconciliationEvent}</p>}
              </details>
              {installation.installation.lastError && <details><summary>Installation diagnostics</summary><p>{installation.installation.lastError}</p></details>}
            </div>
          )}
          {installation?.installation.state === "failed" && recovery && (
            <div className="recovery-panel">
              <strong>Installation interrupted</strong>
              <p>{recovery.vmName ?? "Managed VM"}</p>
              {recovery.previousProblem && <p><strong>Previous problem:</strong> {recovery.previousProblem}</p>}
              <div className="security-summary">
                <span>Current virtualization: {recovery.virtualizationReady ? "Ready" : "Not ready"}</span>
                <span>Firmware virtualization: {recovery.firmwareVirtualization}</span>
                <span>VirtualBox execution: {recovery.virtualBoxExecution === "yes" ? "available" : recovery.virtualBoxExecution}</span>
                <span>VM configuration: {recovery.vmConfigurationValid ? "Valid" : "Invalid"}</span>
                <span>Recovery state: {recoveryClassificationLabel(recovery.classification)}</span>
              </div>
              {!recovery.canResume && <p>{recovery.reason ?? "This managed VM cannot safely resume."}</p>}
              {recovery.canResume && !showResume && (
                <button className="primary-action" disabled={busy} onClick={recovery.credentialsRequired ? onOpenResume : onResume}>Resume installation</button>
              )}
              {recovery.canResume && recovery.credentialsRequired && showResume && (
                <div className="resume-form">
                  <p>The previous credentials were not stored. Enter them again to continue unattended setup.</p>
                  <label>Windows username<input autoComplete="off" value={form.installation.username} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, username: event.target.value } }))} /></label>
                  <label>Password<input type="password" autoComplete="new-password" value={form.installation.password} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, password: event.target.value } }))} /></label>
                  <label>Computer name<input value={form.installation.computerName} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, computerName: event.target.value } }))} /></label>
                  <label>DNS domain<input value={form.installation.domainName} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, domainName: event.target.value } }))} /></label>
                  <small>Full hostname: {fullHostname}</small>
                  <details><summary>Optional product key and recovery details</summary>
                    <label>Product key<input type="password" autoComplete="off" value={form.installation.productKey ?? ""} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, productKey: event.target.value || null } }))} /></label>
                    <small>UUID: {recovery.vmId}<br />Profile: {recovery.virtualBoxOsTypeId} · {recovery.virtualBoxOsDescription}<br />ISO: {recovery.isoPath}<br />System disk: {recovery.systemDiskPath}</small>
                  </details>
                  <div className="runtime-actions"><button className="primary-action" disabled={busy || !form.installation.password} onClick={onResume}>Resume installation</button><button className="secondary-button" disabled={busy} onClick={onCancelResume}>Cancel</button></div>
                </div>
              )}
              {recovery.previousFailure && <details><summary>Previous failure diagnostics</summary><p>{recovery.previousFailure}</p></details>}
            </div>
          )}
          {runtime?.message && <p className="runtime-message error-text">{runtime.message}</p>}
          <div className="runtime-actions">
            {!running && <button className="primary-action" disabled={busy || !selectedVmId} onClick={onStart}>Start Barnen</button>}
            {running && <button className="primary-action" disabled={busy} onClick={onStop}>Stop Barnen</button>}
            {running && runtime?.inputIsolation.displayMode === "seatDisplayLocked" && <button className="secondary-button" disabled={busy} onClick={onAllowHostInput}>Allow host input temporarily</button>}
            {running && runtime?.inputIsolation.displayMode === "normalVirtualBox" && <button className="primary-action" disabled={busy} onClick={onLockSeatInput}>Lock seat input</button>}
            <button className="recovery-action" disabled={busy || !selectedVmId} onClick={onRelease}>Release Barnen Devices</button>
            <button className="secondary-button" disabled={busy || running} onClick={onOpenCreation}>Create Barnen VM</button>
          </div>
          <p className="capture-warning">Start uses the configured route per device. Mouse USB passthrough remains enabled; keyboard USB passthrough is disabled by default and is never retried without explicit safety clearance.</p>
          {showCreation && proposal && selectedProfile && (
            <div className="creation-form">
              <h4>Create Windows VM</h4>
              <p>Host: {Math.round(proposal.hostMemoryMb / 1024)} GB RAM · {proposal.hostLogicalCpus} logical CPUs. MultiSeat Lite will prepare and start VirtualBox's unattended Windows installation. Windows itself is not bundled.</p>
              <label>Name<input value={form.name} onChange={(event) => onFormChange((current) => ({ ...current, name: event.target.value }))} /></label>
              <label>Operating system<select value={form.windowsVersion} onChange={(event) => onFormChange((current) => ({ ...current, windowsVersion: event.target.value as ManagedWindowsVersion }))}>
                {proposal.profiles.map((profile) => <option value={profile.windowsVersion} key={profile.windowsVersion}>{profile.displayName}</option>)}
              </select></label>
              <div className="security-summary">
                <strong>Firmware/security: {selectedProfile.windowsVersion === "windows10" ? "Windows 10 compatible" : "Windows 11 requirements"}</strong>
                <span>Firmware: {selectedProfile.firmware}</span>
                <span>TPM 2.0: {selectedProfile.tpmRequired ? "Required" : "Not required"}</span>
                <span>Secure Boot: {selectedProfile.secureBootRequired ? "Required" : "Not required"}</span>
              </div>
              <label>Windows ISO path<div className="file-picker"><input readOnly placeholder="Select a Windows ISO…" value={form.isoPath} /><button className="secondary-button" onClick={onSelectIso}>Browse…</button></div></label>
              <div className="resource-grid account-grid">
                <label>Local account<input autoComplete="off" value={form.installation.username} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, username: event.target.value } }))} /></label>
                <label>Password<input type="password" autoComplete="new-password" value={form.installation.password} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, password: event.target.value } }))} /></label>
                <label>Computer name<input value={form.installation.computerName} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, computerName: event.target.value } }))} /></label>
              </div>
              <div className="resource-grid">
                <label>RAM (MB)<input type="number" min="2048" value={form.memoryMb} onChange={(event) => onFormChange((current) => ({ ...current, memoryMb: Number(event.target.value) }))} /></label>
                <label>CPUs<input type="number" min="1" value={form.cpus} onChange={(event) => onFormChange((current) => ({ ...current, cpus: Number(event.target.value) }))} /></label>
                <label>Disk (GB)<input type="number" min="32" value={form.diskGb} onChange={(event) => onFormChange((current) => ({ ...current, diskGb: Number(event.target.value) }))} /></label>
              </div>
              <label className="checkbox-label"><input type="checkbox" checked={form.installation.installGuestAdditions} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, installGuestAdditions: event.target.checked } }))} />Install bundled VirtualBox Guest Additions automatically</label>
              <details className="advanced-install">
                <summary>Advanced installation settings</summary>
                <div className="resource-grid">
                  <label>Locale<input value={form.installation.locale} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, locale: event.target.value } }))} /></label>
                  <label>Language<input value={form.installation.language} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, language: event.target.value } }))} /></label>
                  <label>Country<input value={form.installation.country} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, country: event.target.value } }))} /></label>
                </div>
                <label>Time zone (empty uses host)<input value={form.installation.timeZone} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, timeZone: event.target.value } }))} /></label>
                <label>DNS domain<input value={form.installation.domainName} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, domainName: event.target.value } }))} /></label>
                <small>Full hostname: {fullHostname}</small>
                <label>Product key (optional)<input type="password" autoComplete="off" value={form.installation.productKey ?? ""} onChange={(event) => onFormChange((current) => ({ ...current, installation: { ...current.installation, productKey: event.target.value || null } }))} /></label>
              </details>
              <div className="creation-preview" aria-label="VirtualBox configuration preview">
                <PreviewFact label="VM name" value={form.name} />
                <PreviewFact label="Guest OS" value={`${selectedProfile.guestOsDescription} · ID ${selectedProfile.virtualBoxOsTypeId}`} />
                <PreviewFact label="RAM" value={`${form.memoryMb} MB`} />
                <PreviewFact label="CPUs" value={String(form.cpus)} />
                <PreviewFact label="Disk" value={`${form.diskGb} GB · ${selectedProfile.storageController}`} />
                <PreviewFact label="Firmware" value={selectedProfile.firmware} />
                <PreviewFact label="TPM" value={selectedProfile.tpm} />
                <PreviewFact label="Secure Boot" value={selectedProfile.secureBootRequired ? "Microsoft KEK/DB + VirtualBox platform key" : "Not required"} />
                <PreviewFact label="USB" value={selectedProfile.usbController} />
                <PreviewFact label="Graphics" value={`${selectedProfile.graphicsController} · ${selectedProfile.vramMb} MB VRAM`} />
                <PreviewFact label="Network" value={selectedProfile.network} />
                <PreviewFact label="Audio" value={selectedProfile.audioOutputEnabled ? "Output enabled" : "Disabled"} />
                <PreviewFact label="Windows ISO" value={form.isoPath || "Not selected"} />
                <PreviewFact label="Local account" value={form.installation.username || "Not set"} />
                <PreviewFact label="Computer name" value={form.installation.computerName || "Not set"} />
                <PreviewFact label="DNS domain" value={form.installation.domainName || "Not set"} />
                <PreviewFact label="Full hostname" value={fullHostname} />
                <PreviewFact label="Region" value={`${form.installation.locale} · ${form.installation.timeZone || "host time zone"}`} />
                <PreviewFact
                  label="Guest Additions"
                  value={selectedProfile.guestAdditions.bundledIsoPath
                    ? form.installation.installGuestAdditions ? "Automatic installation enabled" : "Automatic installation disabled"
                    : "Bundled ISO not found"}
                />
              </div>
              <div className="runtime-actions"><button className="primary-action" disabled={busy || !form.isoPath.trim() || !form.installation.password} onClick={onCreate}>Create and install</button><button className="secondary-button" disabled={busy} onClick={onCancelCreation}>Cancel</button></div>
            </div>
          )}
        </>
      )}
    </section>
  );
}

function RuntimeComponent({ label, value, healthy }: { label: string; value: string; healthy: boolean }) {
  return <div><span>{label}</span><strong className={healthy ? "healthy" : ""}>{value}</strong></div>;
}

function PreviewFact({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong title={value}>{value}</strong></div>;
}

function windowsStatusLabel(status: ManagedInstallationSnapshot["installation"]["observed"]["windows"]) {
  return ({ unknown: "Unknown", installing: "Installing", installed: "✓ Installed" })[status];
}

function guestAdditionsStatusLabel(status: ManagedInstallationSnapshot["installation"]["observed"]["guestAdditions"]) {
  return ({
    unknown: "Unknown",
    unavailable: "Communication unavailable",
    partialEvidence: "Partial host-side evidence",
    communicating: "✓ Communication available",
    installedVersionKnown: "✓ Installed",
    repairRequired: "Repair required",
  })[status];
}

function managedStatusLabel(status: ManagedInstallationSnapshot["installation"]["observed"]["managed"]) {
  return ({
    installing: "Installing",
    windowsInstalled: "Windows installed",
    waitingForGuestAdditions: "Waiting for Guest Additions",
    operationalUnverified: "Operational state unverified",
    degraded: "Degraded guest communication",
    stopped: "Stopped",
    saved: "Saved",
    ready: "Ready",
    failed: "Needs attention",
  })[status];
}

function usbHealthLabel(health: SeatRuntimeSnapshot["keyboard"]["usb"]["health"]) {
  return ({
    notResolved: "Not resolved",
    hostBusy: "Host busy",
    attachRequested: "Attach requested",
    captured: "Captured",
    disappearedAfterAttach: "Disappeared after attach",
    physicalDeviceDisconnected: "Physically disconnected",
    released: "Released",
    ambiguous: "Ambiguous",
    failed: "Failed",
  })[health];
}

function routingStrategyLabel(strategy: SeatRuntimeSnapshot["keyboard"]["routingStrategy"]) {
  return ({
    virtualBoxUsbPassthrough: "VirtualBox USB passthrough",
    nativeKeyboardRouting: "Native keyboard routing",
    disabled: "Disabled",
  })[strategy];
}

function routingStatusLabel(component: SeatRuntimeSnapshot["keyboard"]) {
  return ({
    notImplemented: "Not implemented",
    prototypeInactive: "Prototype / Not active",
    prototypeActive: "Prototype / Active",
    active: component.routingStrategy === "virtualBoxUsbPassthrough" ? usbHealthLabel(component.usb.health) : "Active",
    error: component.routingStrategy === "virtualBoxUsbPassthrough" ? usbHealthLabel(component.usb.health) : "Error",
    disabled: "Disabled",
  })[component.routingStatus];
}

function safetyLabel(safety: SeatRuntimeSnapshot["keyboard"]["usbPassthroughSafety"]) {
  return ({
    supported: "Supported",
    unverified: "Unverified",
    knownProblematic: "Known problematic",
    disabled: "Disabled",
  })[safety];
}

function UsbRuntimeDiagnostic({ label, component }: { label: string; component: SeatRuntimeSnapshot["keyboard"] }) {
  const usb = component.usb;
  return (
    <details className="usb-diagnostic">
      <summary>{label}: {usbHealthLabel(usb.health)}</summary>
      <p>
        Windows physical device: {usb.windowsPhysicalDevice}<br />
        USB parent: {usb.usbParent}<br />
        VirtualBox host device: {usb.virtualBoxHostDevice}{usb.currentVirtualBoxState ? ` · ${usb.currentVirtualBoxState}` : ""}<br />
        Attach command: {usb.attachCommand}<br />
        Guest visibility: {usb.guestVisibility}<br />
        Runtime UUID: {usb.runtimeUuid ?? "Unknown"}<br />
        Runtime address: {usb.runtimeAddress ?? "Unknown"}<br />
        Before attach: {usb.stateBeforeAttach ?? "Unknown"}<br />
        Immediately after: {usb.stateImmediatelyAfterAttach ?? "Missing / unknown"}<br />
        After retry: {usb.stateAfterRetry ?? "Missing / unknown"}
      </p>
      {usb.probeError && <small className="runtime-warning">{usb.probeError}</small>}
    </details>
  );
}

function installationPercent(state: ManagedInstallationSnapshot["installation"]["state"]) {
  return ({ notCreated: 0, creating: 10, installingWindows: 35, rebooting: 65, installingGuestAdditions: 80, waitingForGuest: 90, ready: 100, failed: 100 })[state];
}

function installationStage(state: ManagedInstallationSnapshot["installation"]["state"]) {
  return ({ notCreated: 0, creating: 0, installingWindows: 2, rebooting: 2, installingGuestAdditions: 3, waitingForGuest: 4, ready: 5, failed: 4 })[state];
}

function recoveryClassificationLabel(classification: ManagedVmRecoveryStatus["classification"]) {
  return ({
    recoverablePreBoot: "Recoverable before preparation",
    recoverablePreparedInstall: "Prepared installation can resume",
    installationInProgress: "Installation in progress",
    ready: "Ready",
    unrecoverable: "Unrecoverable",
    unknown: "Unknown",
  })[classification];
}

function SeatSlot({
  slot,
  assignment,
  identifiedDevice,
  usbMapping,
  disabled,
  onUnassign,
}: {
  slot: VisibleSlot;
  assignment: ResolvedAssignment | null;
  identifiedDevice: IdentifiedInputDevice | null;
  usbMapping: BackendProbeSnapshot["usbCorrelations"][number] | null;
  disabled: boolean;
  onUnassign: () => void;
}) {
  const details = slotDetails[slot];
  return (
    <div className={`seat-slot ${assignment ? "filled" : ""} ${assignment && isIdentified(assignment.id, assignment.containerId, identifiedDevice, slot) ? "identified" : ""}`}>
      <div className={`device-icon ${slot}`}>{details.icon}</div>
      <div className="slot-copy">
        <span>{details.label}</span>
        {assignment ? (
          <>
            <strong>{assignment.friendlyName ?? "Configured device"}</strong>
            <small>{assignment.secondary ?? assignment.id}</small>
            {slot !== "display" && usbMapping && (
              <small title={usbMapping.reason}>
                VirtualBox USB mapping: {usbMappingLabel(usbMapping.state)}
              </small>
            )}
          </>
        ) : (
          <em>Not assigned</em>
        )}
      </div>
      {assignment && (
        <div className="slot-status">
          {isIdentified(assignment.id, assignment.containerId, identifiedDevice, slot) && (
            <span className="this-device-badge">This device</span>
          )}
          <span className={`state ${assignment.availability}`}>{statusLabel(assignment.availability)}</span>
          <button disabled={disabled} onClick={onUnassign}>Unassign</button>
        </div>
      )}
    </div>
  );
}

function BackendFact({ label, value }: { label: string; value: string }) {
  return (
    <div className="backend-fact">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function monitorName(instanceId: string, fallback: string | null, hardware: HardwareSnapshot) {
  const hardwareCode = instanceId.split("\\")[1]?.toLowerCase();
  const displayConfigName = hardware.displayConfigTargets.find((target) =>
    hardwareCode && target.monitorDevicePath?.toLowerCase().includes(hardwareCode),
  )?.friendlyName;
  return displayConfigName ?? fallback ?? "PnP monitor";
}

function statusLabel(status: ResolvedAssignment["availability"]) {
  if (status === "present") return "Connected";
  if (status === "ambiguous") return "Ambiguous";
  return "Missing";
}

function usbMappingLabel(state: BackendProbeSnapshot["usbCorrelations"][number]["state"]) {
  if (state === "exact") return "Exact";
  if (state === "unambiguous") return "Unambiguous";
  if (state === "ambiguous") return "Ambiguous";
  return "Unavailable";
}

function hasAsterRuntimeEvidence(aster: HardwareSnapshot["environment"]["aster"]) {
  return aster.relatedServiceRunning || aster.mutEnxRawInputVisible;
}

function isIdentified(
  id: string,
  containerId: string | null,
  identified: IdentifiedInputDevice | null,
  expectedType?: VisibleSlot,
) {
  if (!identified || expectedType === "display" || (expectedType && expectedType !== identified.deviceType)) return false;
  return id.toLowerCase() === identified.stablePhysicalDeviceId.toLowerCase()
    || Boolean(containerId && identified.containerId && containerId.toLowerCase() === identified.containerId.toLowerCase());
}

export default App;
