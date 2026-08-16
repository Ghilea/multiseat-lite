export type AssignmentSlot = "display" | "keyboard" | "mouse" | "audioOutput";
export type AssignmentAvailability = "present" | "missing" | "ambiguous";

export type PnpMonitor = {
  instanceId: string;
  friendlyName: string | null;
  manufacturer: string | null;
  hardwareIds: string[];
};

export type LogicalInputDevice = {
  id: string;
  devicePath: string | null;
  instanceId: string;
  containerId: string | null;
  friendlyName: string | null;
  manufacturer: string | null;
  hardwareIds: string[];
  enumerator: string | null;
  deviceType: "keyboard" | "mouse" | "hid";
  vendorId: string | null;
  productId: string | null;
  usbParentInstanceId: string | null;
  serialNumber: string | null;
};

export type InputCapabilities = {
  keyboard: boolean;
  mouse: boolean;
};

export type InputDevice = {
  id: string;
  containerId: string | null;
  friendlyName: string | null;
  manufacturer: string | null;
  vendorId: string | null;
  productId: string | null;
  usbParentInstanceId: string | null;
  serialNumber: string | null;
  capabilities: InputCapabilities;
  logicalNodes: LogicalInputDevice[];
  rawInputPaths: string[];
  present: boolean;
};

export type DisplayConfigTarget = {
  id: string;
  friendlyName: string | null;
  monitorDevicePath: string | null;
  active: boolean;
  available: boolean;
};

export type HardwareSnapshot = {
  pnpMonitors: PnpMonitor[];
  inputDevices: InputDevice[];
  logicalInputDevices: LogicalInputDevice[];
  displayConfigTargets: DisplayConfigTarget[];
  environment: {
    currentSessionId: number;
    aster: AsterEnvironmentStatus;
  };
};

export type ServiceRuntimeState =
  | "notInstalled"
  | "stopped"
  | "startPending"
  | "stopPending"
  | "running"
  | "continuePending"
  | "pausePending"
  | "paused"
  | "unknown";

export type AsterEnvironmentStatus = {
  installationDetected: boolean;
  relatedServiceInstalled: boolean;
  relatedServiceRunning: boolean;
  mutEnxDeviceNodePresent: boolean;
  mutEnxRawInputVisible: boolean;
  registryMarkersPresent: boolean;
  registryMarkers: string[];
  relatedServices: Array<{
    name: string;
    installed: boolean;
    running: boolean;
    state: ServiceRuntimeState;
  }>;
  workplaceState: "active" | "inactive" | "unknown";
};

export type ResolvedAssignment = {
  id: string;
  containerId: string | null;
  availability: AssignmentAvailability;
  friendlyName: string | null;
  secondary: string | null;
};

export type ResolvedSeat = {
  id: string;
  name: string;
  devices: Record<AssignmentSlot, ResolvedAssignment | null>;
};

export type SeatConfig = {
  id: string;
  name: string;
  devices: Record<AssignmentSlot, string | null>;
};

export type ApplicationConfigSnapshot = {
  configuration: {
    version: number;
    seats: SeatConfig[];
    backends: {
      inputRouting: Record<string, SeatInputRouting>;
      deviceSafety: DeviceBackendSafetyRecord[];
      virtualBox: {
        seats: Record<string, {
          vmId: string;
          managedByMultiseat: boolean;
          mouseCapturePolicyOwned: boolean;
          installation: ManagedVmInstallation | null;
        }>;
      };
    };
  };
  resolvedSeats: ResolvedSeat[];
  loadWarning: string | null;
  configurationPath: string;
};

export type InputRoutingStrategy = "virtualBoxUsbPassthrough" | "nativeKeyboardRouting" | "disabled";
export type InputRoutingRuntimeStatus = "notImplemented" | "prototypeInactive" | "prototypeActive" | "active" | "error" | "disabled";
export type UsbPassthroughSafety = "supported" | "unverified" | "knownProblematic" | "disabled";

export type SeatInputRouting = {
  keyboard: InputRoutingStrategy;
  mouse: InputRoutingStrategy;
};

export type DeviceBackendSafetyRecord = {
  physicalDeviceId: string;
  vendorId: string | null;
  productId: string | null;
  backend: "virtualBoxUsbPassthrough";
  safety: UsbPassthroughSafety;
  reason: string;
  observed: string | null;
};

export type VirtualMachine = {
  name: string;
  uuid: string;
  state: string;
  guestOsDescription: string | null;
  usbXhciEnabled: boolean | null;
};

export type VirtualMachineList = {
  machines: VirtualMachine[];
  warnings: string[];
};

export type SeatRuntimeStatus =
  | "unsupported"
  | "notConfigured"
  | "stopped"
  | "starting"
  | "running"
  | "partiallyRunning"
  | "stopping"
  | "error";

export type RuntimeComponentStatus = {
  configured: boolean;
  attached: boolean;
  physicalDeviceId: string | null;
  runtimeUuid: string | null;
  mapping: UsbCorrelationState | null;
  routingStrategy: InputRoutingStrategy;
  routingStatus: InputRoutingRuntimeStatus;
  usbPassthroughSafety: UsbPassthroughSafety;
  safetyReason: string | null;
  usb: {
    windowsPhysicalDevice: "present" | "missing" | "unknown";
    usbParent: "present" | "missing" | "unknown";
    virtualBoxHostDevice: "present" | "missing" | "unknown";
    resolvedUsbParent: string | null;
    runtimeUuid: string | null;
    runtimeAddress: string | null;
    stateBeforeAttach: string | null;
    attachCommand: "notRequested" | "success" | "failure";
    stateImmediatelyAfterAttach: string | null;
    stateAfterRetry: string | null;
    currentVirtualBoxState: string | null;
    guestVisibility: "verified" | "missing" | "unknown";
    health: "notResolved" | "hostBusy" | "attachRequested" | "captured" | "disappearedAfterAttach" | "physicalDeviceDisconnected" | "released" | "ambiguous" | "failed";
    probeError: string | null;
  };
};

export type SeatRuntimeSnapshot = {
  seatId: string;
  status: SeatRuntimeStatus;
  vmId: string | null;
  vmState: string | null;
  vmStartedByMultiseat: boolean;
  keyboard: RuntimeComponentStatus;
  mouse: RuntimeComponentStatus;
  inputIsolation: {
    displayMode: "normalVirtualBox" | "seatDisplayLocked";
    mouseCapturePolicy: string | null;
    mouseCaptureDisabled: boolean;
    mouseCapturePolicyOwned: boolean;
    mouseCapturePolicyError: string | null;
    mouseIntegration: {
      requested: "enabled" | "disabled" | "unknown";
      observed: "enabled" | "disabled" | "unknown";
      control: "supported" | "manualRequired";
      ownedByMultiseat: boolean;
      message: string | null;
    };
    mouseIsolation: "active" | "inactive" | "error" | "unverified";
    focusProtectionActive: boolean;
    focusProtectionLocked: boolean;
    focusRestorationCount: number;
    focusProtectionError: string | null;
    hostGuiKeyboardIsolation: "bestEffort";
    dennisKeyboardAttached: boolean;
    dennisMouseAttached: boolean;
  };
  message: string | null;
  logs: Array<{
    action: string;
    outcome: string;
    physicalDeviceId: string | null;
    runtimeUuid: string | null;
    detail: string;
  }>;
};

export type KeyboardRoutingDiagnosticStatus = {
  active: boolean;
  targetPhysicalDeviceId: string | null;
  currentSessionId: number;
  realGuestInjectionEnabled: boolean;
  hostInputSuppression: string;
  transport: "diagnosticOnly" | "virtualBoxScancodePrototype";
};

export type KeyboardRoutingDiagnosticEvent = {
  action: "keyDown" | "keyUp" | "releaseAll" | "backendError";
  event: {
    sourcePhysicalDeviceId: string;
    rawInputDevicePath: string;
    scanCode: number;
    virtualKey: number;
    transition: "down" | "up";
    extendedE0: boolean;
    extendedE1: boolean;
    modifier: "leftShift" | "rightShift" | "leftControl" | "rightControl" | "leftAlt" | "rightAlt" | "leftWindows" | "rightWindows" | null;
  } | null;
  detail: string;
};

export type SeatOperationResult = {
  success: boolean;
  runtime: SeatRuntimeSnapshot;
  errors: string[];
  rollbackAttempted: boolean;
};

export type VmCreationProposal = {
  hostMemoryMb: number;
  hostLogicalCpus: number;
  memoryMb: number;
  cpus: number;
  diskGb: number;
  defaultWindowsVersion: ManagedWindowsVersion;
  profiles: ManagedWindowsVmProfile[];
  installationDefaults: {
    username: string;
    computerName: string;
    domainName: string;
    hostname: string;
    locale: string;
    language: string;
    country: string;
    timeZone: string;
    installGuestAdditions: boolean;
  };
};

export type ManagedWindowsVersion = "windows10" | "windows11";

export type ManagedWindowsVmProfile = {
  windowsVersion: ManagedWindowsVersion;
  displayName: string;
  virtualBoxOsTypeId: string;
  guestOsDescription: string;
  architecture: string;
  firmware: string;
  tpm: string;
  tpmRequired: boolean;
  ioApicEnabled: boolean;
  secureBoot: "notRequired" | "willConfigure" | "configured";
  secureBootRequired: boolean;
  usbController: string;
  graphicsController: string;
  vramMb: number;
  network: string;
  audioOutputEnabled: boolean;
  storageController: string;
  guestAdditions: {
    state: "notInstalled" | "installationPending" | "installed" | "unknown";
    bundledIsoPath: string | null;
    automaticInstallationSupported: boolean;
  };
};

export type ManagedInstallState =
  | "notCreated"
  | "creating"
  | "installingWindows"
  | "rebooting"
  | "installingGuestAdditions"
  | "waitingForGuest"
  | "ready"
  | "failed";

export type GuestAdditionsInstallState =
  | "notInstalled"
  | "installing"
  | "installed"
  | "failed"
  | "unknown";

export type ManagedVmInstallation = {
  windowsVersion: ManagedWindowsVersion;
  virtualBoxOsTypeId: string;
  computerName: string;
  domainName: string;
  state: ManagedInstallState;
  guestAdditions: GuestAdditionsInstallState;
  guestAdditionsVersion: string | null;
  lastError: string | null;
  observed: ManagedInstallationObservation;
};

export type ManagedInstallationObservation = {
  windows: "unknown" | "installing" | "installed";
  guestAdditions: "unknown" | "unavailable" | "partialEvidence" | "communicating" | "installedVersionKnown" | "repairRequired";
  runLevel: "unknown" | "system" | "userland" | "desktop";
  managed: "installing" | "windowsInstalled" | "waitingForGuestAdditions" | "operationalUnverified" | "degraded" | "stopped" | "saved" | "ready" | "failed";
  evidence: {
    lastVmState: string | null;
    runlevelProbes: Array<{
      level: "unknown" | "system" | "userland" | "desktop";
      success: boolean;
      exitCode: number | null;
      stdout: string;
      stderr: string;
      elapsedMs: number;
    }>;
    guestOsProduct: string | null;
    guestAdditionsVersion: string | null;
    guestAddHostVersionLastChecked: string | null;
    resetCounter: number | null;
    reconciliationTimestampUnixMs: number | null;
    reconciliationEvent: string | null;
  };
};

export type ManagedInstallationSnapshot = {
  vmId: string;
  vmState: string;
  installation: ManagedVmInstallation;
  windowsGuestDetected: boolean;
  monitoringComplete: boolean;
};

export type ManagedVmRecoveryStatus = {
  canResume: boolean;
  classification: "recoverablePreBoot" | "recoverablePreparedInstall" | "installationInProgress" | "ready" | "unrecoverable" | "unknown";
  reason: string | null;
  previousFailure: string | null;
  previousProblem: string | null;
  firmwareVirtualization: "yes" | "no" | "unknown";
  virtualBoxExecution: "yes" | "no" | "unknown";
  virtualizationReady: boolean;
  virtualizationBlocker: string | null;
  vmConfigurationValid: boolean;
  unattendedPreparationPresent: boolean;
  unattendedMediaPath: string | null;
  vmId: string;
  vmName: string | null;
  vmState: string | null;
  windowsVersion: ManagedWindowsVersion;
  virtualBoxOsTypeId: string;
  virtualBoxOsDescription: string | null;
  systemDiskPath: string | null;
  isoPath: string | null;
  isoMediumUuid: string | null;
  credentialsRequired: boolean;
  suggestedComputerName: string;
  suggestedDomainName: string;
  suggestedHostname: string;
};

export type AssignableDevice = {
  id: string;
  containerId: string | null;
  slot: Exclude<AssignmentSlot, "audioOutput">;
  capabilities: InputCapabilities | null;
  name: string;
  secondary: string;
};

export type IdentifiedInputDevice = {
  deviceType: "keyboard" | "mouse";
  stablePhysicalDeviceId: string;
  containerId: string | null;
  rawInputDevicePath: string;
  friendlyName: string | null;
  vendorId: string | null;
  productId: string | null;
  currentSessionId: number;
};

export type IdentificationStatus = {
  active: boolean;
  sessionRawInputDevices: number;
  correlatedPhysicalDevices: number;
};

export type UsbCorrelationState = "exact" | "unambiguous" | "ambiguous" | "unavailable";

export type BackendProbeSnapshot = {
  system: {
    productName: string | null;
    edition: string | null;
    displayVersion: string | null;
    build: string | null;
    architecture: string;
    currentSessionId: number;
    hypervisorPresent: boolean;
    firmwareVirtualizationEnabled: "yes" | "no" | "unknown";
    secondLevelAddressTranslation: "yes" | "no" | "unknown";
  };
  virtualBox: {
    installed: boolean;
    vboxManagePath: string | null;
    version: string | null;
    operational: boolean;
    probeError: string | null;
    parserWarnings: string[];
    usbDevices: Array<{
      uuid: string;
      vendorId: string | null;
      productId: string | null;
      revision: string | null;
      port: string | null;
      usbVersionSpeed: string | null;
      manufacturer: string | null;
      product: string | null;
      serialNumber: string | null;
      address: string | null;
      currentState: string | null;
    }>;
  };
  hyperV: {
    featureState: "enabled" | "disabled" | "unavailable" | "unknown";
    featureProbeError: string | null;
    services: Array<{ name: string; installed: boolean; running: boolean; state: string }>;
    hypervisorActive: boolean;
  };
  aster: AsterEnvironmentStatus;
  selectedBackend: "virtualBox" | "hyperV" | "native" | "unsupported" | null;
  usbCorrelations: Array<{
    physicalDeviceId: string;
    containerId: string | null;
    state: UsbCorrelationState;
    matchedUuid: string | null;
    candidateUuids: string[];
    reason: string;
  }>;
};
