#include <initguid.h>
#include "driver.h"

/* Service-only interface for the future privileged service. The filter is
 * still physically incapable of suppressing packets in this build. */
DEFINE_GUID(GUID_DEVINTERFACE_MSKL_KEYBOARD_FILTER,
    0x8423bd6e, 0xcf40, 0x4d1c, 0x9f, 0xf5, 0x48, 0x94, 0x75, 0x80, 0x26, 0x21);

static NTSTATUS MsklQueryIdentity(_In_ WDFDEVICE Device);
static BOOLEAN MsklEqualFixedString(_In_reads_(Capacity) const WCHAR* Left,
    _In_reads_(Capacity) const WCHAR* Right, _In_ SIZE_T Capacity);
static NTSTATUS MsklValidateHeader(_In_ const MSKL_MESSAGE_HEADER* Header,
    _In_ size_t InputLength, _In_ uint32_t Command, _In_ uint16_t ExpectedSize);
static VOID MsklCopyFixedString(_Out_writes_(Capacity) WCHAR* Destination,
    _In_ SIZE_T Capacity, _In_reads_(Capacity) const WCHAR* Source);
static VOID MsklRecordPackets(_In_ PMSKL_DEVICE_CONTEXT Context,
    _In_ PKEYBOARD_INPUT_DATA InputDataStart,
    _In_ PKEYBOARD_INPUT_DATA InputDataEnd);
static VOID MsklClearObservation(_In_ PMSKL_DEVICE_CONTEXT Context);

/* CONNECT_DATA deliberately exposes ClassService as PVOID even though the
 * keyboard class contract defines it as PSERVICE_CALLBACK_ROUTINE. This union
 * keeps the conversion explicit while retaining the exact WDK callback ABI. */
typedef union _MSKL_SERVICE_CALLBACK_POINTER {
    PVOID Opaque;
    PSERVICE_CALLBACK_ROUTINE Typed;
} MSKL_SERVICE_CALLBACK_POINTER;

NTSTATUS
DriverEntry(_In_ PDRIVER_OBJECT DriverObject, _In_ PUNICODE_STRING RegistryPath)
{
    WDF_DRIVER_CONFIG config;
    WDF_DRIVER_CONFIG_INIT(&config, MsklEvtDeviceAdd);
    return WdfDriverCreate(DriverObject, RegistryPath, WDF_NO_OBJECT_ATTRIBUTES,
        &config, WDF_NO_HANDLE);
}

NTSTATUS
MsklEvtDeviceAdd(_In_ WDFDRIVER Driver, _Inout_ PWDFDEVICE_INIT DeviceInit)
{
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES lockAttributes;
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDF_FILEOBJECT_CONFIG fileConfig;
    WDF_PNPPOWER_EVENT_CALLBACKS pnpCallbacks;
    WDFDEVICE device;
    PMSKL_DEVICE_CONTEXT context;
    NTSTATUS status;

    UNREFERENCED_PARAMETER(Driver);
    WdfFdoInitSetFilter(DeviceInit);

    WDF_FILEOBJECT_CONFIG_INIT(&fileConfig, MsklEvtFileCreate,
        WDF_NO_EVENT_CALLBACK, MsklEvtFileCleanup);
    WdfDeviceInitSetFileObjectConfig(DeviceInit, &fileConfig,
        WDF_NO_OBJECT_ATTRIBUTES);

    WDF_PNPPOWER_EVENT_CALLBACKS_INIT(&pnpCallbacks);
    pnpCallbacks.EvtDeviceReleaseHardware = MsklEvtDeviceReleaseHardware;
    WdfDeviceInitSetPnpPowerEventCallbacks(DeviceInit, &pnpCallbacks);

    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, MSKL_DEVICE_CONTEXT);
    status = WdfDeviceCreate(&DeviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    context = MsklGetDeviceContext(device);
    RtlZeroMemory(context, sizeof(*context));
    context->LastNtStatus = STATUS_SUCCESS;

    WDF_OBJECT_ATTRIBUTES_INIT(&lockAttributes);
    lockAttributes.ParentObject = device;
    status = WdfSpinLockCreate(&lockAttributes, &context->RingLock);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    status = MsklQueryIdentity(device);
    context->LastNtStatus = status;
    if (!NT_SUCCESS(status)) {
        /* Identity is diagnostic in this pass-through build. Failure must not
         * prevent the keyboard stack from loading. */
        RtlZeroMemory(context->DeviceInstanceId,
            sizeof(context->DeviceInstanceId));
        RtlZeroMemory(context->ContainerId, sizeof(context->ContainerId));
    }

    status = WdfDeviceCreateDeviceInterface(device,
        &GUID_DEVINTERFACE_MSKL_KEYBOARD_FILTER, NULL);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig,
        WdfIoQueueDispatchParallel);
    queueConfig.EvtIoInternalDeviceControl = MsklEvtIoInternalDeviceControl;
    queueConfig.EvtIoDeviceControl = MsklEvtIoDeviceControl;
    return WdfIoQueueCreate(device, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES,
        WDF_NO_HANDLE);
}

NTSTATUS
MsklEvtDeviceReleaseHardware(_In_ WDFDEVICE Device,
    _In_ WDFCMRESLIST ResourcesTranslated)
{
    PMSKL_DEVICE_CONTEXT context = MsklGetDeviceContext(Device);
    UNREFERENCED_PARAMETER(ResourcesTranslated);
    InterlockedIncrement64(&context->PnpDisconnectEvents);
    MsklClearObservation(context);
    context->TargetResolved = FALSE;
    context->Connected = FALSE;
    RtlZeroMemory(&context->UpperConnectData, sizeof(context->UpperConnectData));
    return STATUS_SUCCESS;
}

VOID
MsklEvtFileCreate(_In_ WDFDEVICE Device, _In_ WDFREQUEST Request,
    _In_ WDFFILEOBJECT FileObject)
{
    PMSKL_DEVICE_CONTEXT context = MsklGetDeviceContext(Device);
    UNREFERENCED_PARAMETER(FileObject);
    InterlockedIncrement(&context->OpenServiceHandles);
    WdfRequestComplete(Request, STATUS_SUCCESS);
}

VOID
MsklEvtFileCleanup(_In_ WDFFILEOBJECT FileObject)
{
    WDFDEVICE device = WdfFileObjectGetDevice(FileObject);
    PMSKL_DEVICE_CONTEXT context = MsklGetDeviceContext(device);
    if (InterlockedDecrement(&context->OpenServiceHandles) <= 0) {
        InterlockedExchange(&context->OpenServiceHandles, 0);
        MsklClearObservation(context);
    }
}

VOID
MsklEvtIoInternalDeviceControl(_In_ WDFQUEUE Queue, _In_ WDFREQUEST Request,
    _In_ size_t OutputBufferLength, _In_ size_t InputBufferLength,
    _In_ ULONG IoControlCode)
{
    WDFDEVICE device = WdfIoQueueGetDevice(Queue);
    PMSKL_DEVICE_CONTEXT context = MsklGetDeviceContext(device);
    PCONNECT_DATA connectData;
    NTSTATUS status;

    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(InputBufferLength);

    if (IoControlCode == IOCTL_INTERNAL_KEYBOARD_CONNECT) {
        InterlockedIncrement64(&context->ConnectRequests);
        status = WdfRequestRetrieveInputBuffer(Request, sizeof(CONNECT_DATA),
            (PVOID*)&connectData, NULL);
        if (!NT_SUCCESS(status)) {
            context->LastNtStatus = status;
            WdfRequestComplete(Request, status);
            return;
        }
        if (context->Connected) {
            context->LastNtStatus = STATUS_SHARING_VIOLATION;
            WdfRequestComplete(Request, STATUS_SHARING_VIOLATION);
            return;
        }
        context->UpperConnectData = *connectData;
        connectData->ClassDeviceObject = WdfDeviceWdmGetDeviceObject(device);
        MSKL_SERVICE_CALLBACK_POINTER callback;
        callback.Typed = MsklServiceCallback;
        connectData->ClassService = callback.Opaque;
        context->Connected = TRUE;
    }

    if (!WdfRequestSend(Request, WdfDeviceGetIoTarget(device),
            WDF_NO_SEND_OPTIONS)) {
        status = WdfRequestGetStatus(Request);
        context->LastNtStatus = status;
        WdfRequestComplete(Request, status);
    }
}

VOID
MsklEvtIoDeviceControl(_In_ WDFQUEUE Queue, _In_ WDFREQUEST Request,
    _In_ size_t OutputBufferLength, _In_ size_t InputBufferLength,
    _In_ ULONG IoControlCode)
{
    WDFDEVICE device = WdfIoQueueGetDevice(Queue);
    PMSKL_DEVICE_CONTEXT context = MsklGetDeviceContext(device);
    PMSKL_MESSAGE_HEADER header;
    NTSTATUS status;
    size_t information = 0;

    status = WdfRequestRetrieveInputBuffer(Request, sizeof(MSKL_MESSAGE_HEADER),
        (PVOID*)&header, NULL);
    if (!NT_SUCCESS(status)) {
        goto Complete;
    }

    switch (IoControlCode) {
    case IOCTL_MSKL_ENABLE:
        status = MsklValidateHeader(header, InputBufferLength,
            MsklCommandEnableSuppression, sizeof(MSKL_MESSAGE_HEADER));
        if (NT_SUCCESS(status)) {
            /* Milestone invariant: no state exists that can suppress input. */
            status = STATUS_NOT_SUPPORTED;
        }
        break;

    case IOCTL_MSKL_DISABLE:
        status = MsklValidateHeader(header, InputBufferLength,
            MsklCommandDisableSuppression, sizeof(MSKL_MESSAGE_HEADER));
        break;

    case IOCTL_MSKL_HEARTBEAT:
        status = MsklValidateHeader(header, InputBufferLength,
            MsklCommandHeartbeat, sizeof(MSKL_MESSAGE_HEADER));
        if (NT_SUCCESS(status)) {
            context->LastHeartbeatInterruptTime = KeQueryInterruptTime();
        }
        break;

    case IOCTL_MSKL_SET_TARGET:
    {
        PMSKL_SET_TARGET_REQUEST target;
        ULONG digestIndex;
        BOOLEAN digestPresent = FALSE;
        status = MsklValidateHeader(header, InputBufferLength,
            MsklCommandSetTargetDevice, sizeof(MSKL_SET_TARGET_REQUEST));
        if (!NT_SUCCESS(status)) {
            break;
        }
        target = (PMSKL_SET_TARGET_REQUEST)header;
        for (digestIndex = 0;
            digestIndex < sizeof(target->physical_identity_digest);
            digestIndex++) {
            if (target->physical_identity_digest[digestIndex] != 0) {
                digestPresent = TRUE;
                break;
            }
        }
        if (!digestPresent) {
            status = STATUS_INVALID_PARAMETER;
            break;
        }
        if (!MsklEqualFixedString(target->collection_instance_id,
                context->DeviceInstanceId, MSKL_MAX_INSTANCE_ID_CHARS) ||
            !MsklEqualFixedString(target->container_id, context->ContainerId,
                MSKL_MAX_CONTAINER_ID_CHARS)) {
            context->TargetResolved = FALSE;
            status = STATUS_NOT_FOUND;
            break;
        }
        MsklCopyFixedString(context->TargetInstanceId,
            MSKL_MAX_INSTANCE_ID_CHARS, target->collection_instance_id);
        context->TargetResolved = TRUE;
        status = STATUS_SUCCESS;
        break;
    }

    case IOCTL_MSKL_SET_OBSERVATION:
    {
        PMSKL_OBSERVATION_REQUEST observation;
        status = MsklValidateHeader(header, InputBufferLength,
            MsklCommandSetObservation, sizeof(MSKL_OBSERVATION_REQUEST));
        if (!NT_SUCCESS(status)) {
            break;
        }
        observation = (PMSKL_OBSERVATION_REQUEST)header;
        if (observation->enabled != 0 && !context->TargetResolved) {
            status = STATUS_INVALID_DEVICE_STATE;
            break;
        }
        MsklClearObservation(context);
        context->ObservationEnabled = observation->enabled != 0;
        status = STATUS_SUCCESS;
        break;
    }

    case IOCTL_MSKL_QUERY:
    {
        PMSKL_STATUS_RESPONSE response;
        uint64_t requestId = header->request_id;
        status = MsklValidateHeader(header, InputBufferLength,
            MsklCommandQueryStatus, sizeof(MSKL_MESSAGE_HEADER));
        if (!NT_SUCCESS(status)) {
            break;
        }
        status = WdfRequestRetrieveOutputBuffer(Request,
            sizeof(MSKL_STATUS_RESPONSE), (PVOID*)&response, NULL);
        if (!NT_SUCCESS(status)) {
            break;
        }
        RtlZeroMemory(response, sizeof(*response));
        response->header.magic = MSKL_PROTOCOL_MAGIC;
        response->header.version = MSKL_PROTOCOL_VERSION;
        response->header.size = (uint16_t)sizeof(*response);
        response->header.command = MsklCommandQueryStatus;
        response->header.request_id = requestId;
        response->driver_loaded = TRUE;
        response->filter_attached = TRUE;
        response->target_resolved = context->TargetResolved;
        response->service_connected = context->OpenServiceHandles > 0;
        response->observation_enabled = context->ObservationEnabled;
        response->suppression_requested = FALSE;
        response->suppression_active = FALSE;
        response->lease_active = FALSE;
        response->fail_open = TRUE;
        response->suppression_capability = MsklSuppressionDisabledByBuild;
        response->fail_open_reason = MsklFailOpenPrototypeLocked;
        response->last_ntstatus = (uint32_t)context->LastNtStatus;
        response->ring_capacity = MSKL_RING_CAPACITY;
        WdfSpinLockAcquire(context->RingLock);
        response->ring_count = context->RingCount;
        WdfSpinLockRelease(context->RingLock);
        response->packets_observed = context->PacketsObserved;
        response->pass_through_packets = context->PassThroughPackets;
        response->connect_requests = context->ConnectRequests;
        response->pnp_disconnect_events = context->PnpDisconnectEvents;
        response->buffer_overflow_count = context->BufferOverflowCount;
        response->last_heartbeat_interrupt_time_100ns =
            context->LastHeartbeatInterruptTime;
        MsklCopyFixedString(response->device_instance_id,
            MSKL_MAX_INSTANCE_ID_CHARS, context->DeviceInstanceId);
        MsklCopyFixedString(response->container_id,
            MSKL_MAX_CONTAINER_ID_CHARS, context->ContainerId);
        MsklCopyFixedString(response->target_instance_id,
            MSKL_MAX_INSTANCE_ID_CHARS, context->TargetInstanceId);
        information = sizeof(*response);
        status = STATUS_SUCCESS;
        break;
    }

    case IOCTL_MSKL_READ_EVENTS:
    {
        PMSKL_EVENT_BATCH batch;
        ULONG index;
        uint64_t requestId = header->request_id;
        status = MsklValidateHeader(header, InputBufferLength,
            MsklCommandReadEvents, sizeof(MSKL_MESSAGE_HEADER));
        if (!NT_SUCCESS(status)) {
            break;
        }
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(MSKL_EVENT_BATCH),
            (PVOID*)&batch, NULL);
        if (!NT_SUCCESS(status)) {
            break;
        }
        RtlZeroMemory(batch, sizeof(*batch));
        batch->header.magic = MSKL_PROTOCOL_MAGIC;
        batch->header.version = MSKL_PROTOCOL_VERSION;
        batch->header.size = (uint16_t)sizeof(*batch);
        batch->header.command = MsklCommandReadEvents;
        batch->header.request_id = requestId;
        WdfSpinLockAcquire(context->RingLock);
        while (batch->event_count < MSKL_EVENT_BATCH_CAPACITY &&
            context->RingCount > 0) {
            index = context->RingHead;
            batch->events[batch->event_count++] = context->Ring[index];
            context->RingHead = (context->RingHead + 1) % MSKL_RING_CAPACITY;
            context->RingCount--;
        }
        WdfSpinLockRelease(context->RingLock);
        information = sizeof(*batch);
        status = STATUS_SUCCESS;
        break;
    }

    default:
        status = STATUS_INVALID_DEVICE_REQUEST;
        break;
    }

Complete:
    context->LastNtStatus = status;
    WdfRequestCompleteWithInformation(Request, status, information);
    UNREFERENCED_PARAMETER(OutputBufferLength);
}

VOID
MsklServiceCallback(_In_ PVOID NormalContext,
    _In_ PVOID SystemArgument1,
    _In_ PVOID SystemArgument2,
    _Inout_ PVOID SystemArgument3)
{
    PDEVICE_OBJECT deviceObject = (PDEVICE_OBJECT)NormalContext;
    PKEYBOARD_INPUT_DATA inputDataStart =
        (PKEYBOARD_INPUT_DATA)SystemArgument1;
    PKEYBOARD_INPUT_DATA inputDataEnd =
        (PKEYBOARD_INPUT_DATA)SystemArgument2;
    PULONG inputDataConsumed = (PULONG)SystemArgument3;
    WDFDEVICE device = WdfWdmDeviceGetWdfDeviceHandle(deviceObject);
    PMSKL_DEVICE_CONTEXT context = MsklGetDeviceContext(device);
    ULONG packetCount = (ULONG)(inputDataEnd - inputDataStart);

    InterlockedAdd64(&context->PacketsObserved, packetCount);
    MsklRecordPackets(context, inputDataStart, inputDataEnd);

    /* UNCONDITIONAL, SINGLE, UNCHANGED FORWARD. There is deliberately no
     * suppression flag, lease, packet-edit, or packet-drop branch. */
    if (context->UpperConnectData.ClassService != NULL) {
        MSKL_SERVICE_CALLBACK_POINTER callback;
        callback.Opaque = context->UpperConnectData.ClassService;
        callback.Typed(context->UpperConnectData.ClassDeviceObject,
            inputDataStart, inputDataEnd, inputDataConsumed);
        InterlockedAdd64(&context->PassThroughPackets, packetCount);
    }
}

static VOID
MsklRecordPackets(_In_ PMSKL_DEVICE_CONTEXT Context,
    _In_ PKEYBOARD_INPUT_DATA InputDataStart,
    _In_ PKEYBOARD_INPUT_DATA InputDataEnd)
{
    PKEYBOARD_INPUT_DATA packet;
    if (!Context->ObservationEnabled || Context->OpenServiceHandles <= 0) {
        return;
    }
    WdfSpinLockAcquire(Context->RingLock);
    for (packet = InputDataStart; packet < InputDataEnd; packet++) {
        ULONG tail;
        PMSKL_KEY_EVENT_COPY copy;
        uint64_t sequence =
            (uint64_t)InterlockedIncrement64(&Context->NextSequence);
        if (Context->RingCount == MSKL_RING_CAPACITY) {
            InterlockedIncrement64(&Context->BufferOverflowCount);
            continue;
        }
        tail = (Context->RingHead + Context->RingCount) % MSKL_RING_CAPACITY;
        copy = &Context->Ring[tail];
        copy->sequence = sequence;
        copy->interrupt_time_100ns = KeQueryInterruptTime();
        copy->unit_id = packet->UnitId;
        copy->make_code = packet->MakeCode;
        copy->flags = packet->Flags;
        copy->reserved = 0;
        Context->RingCount++;
    }
    WdfSpinLockRelease(Context->RingLock);
}

static VOID
MsklClearObservation(_In_ PMSKL_DEVICE_CONTEXT Context)
{
    Context->ObservationEnabled = FALSE;
    WdfSpinLockAcquire(Context->RingLock);
    RtlSecureZeroMemory(Context->Ring, sizeof(Context->Ring));
    Context->RingHead = 0;
    Context->RingCount = 0;
    WdfSpinLockRelease(Context->RingLock);
}

static NTSTATUS
MsklValidateHeader(_In_ const MSKL_MESSAGE_HEADER* Header,
    _In_ size_t InputLength, _In_ uint32_t Command, _In_ uint16_t ExpectedSize)
{
    if (InputLength < ExpectedSize || Header->size != ExpectedSize ||
        Header->magic != MSKL_PROTOCOL_MAGIC || Header->command != Command) {
        return STATUS_INVALID_PARAMETER;
    }
    if (Header->version != MSKL_PROTOCOL_VERSION) {
        return STATUS_REVISION_MISMATCH;
    }
    return STATUS_SUCCESS;
}

static BOOLEAN
MsklEqualFixedString(_In_reads_(Capacity) const WCHAR* Left,
    _In_reads_(Capacity) const WCHAR* Right, _In_ SIZE_T Capacity)
{
    SIZE_T index;
    for (index = 0; index < Capacity; index++) {
        WCHAR left = RtlUpcaseUnicodeChar(Left[index]);
        WCHAR right = RtlUpcaseUnicodeChar(Right[index]);
        if (left != right) {
            return FALSE;
        }
        if (left == L'\0') {
            return TRUE;
        }
    }
    return FALSE;
}

static VOID
MsklCopyFixedString(_Out_writes_(Capacity) WCHAR* Destination,
    _In_ SIZE_T Capacity, _In_reads_(Capacity) const WCHAR* Source)
{
    SIZE_T index;
    if (Capacity == 0) {
        return;
    }
    for (index = 0; index + 1 < Capacity && Source[index] != L'\0'; index++) {
        Destination[index] = Source[index];
    }
    Destination[index] = L'\0';
}

static NTSTATUS
MsklQueryIdentity(_In_ WDFDEVICE Device)
{
    PMSKL_DEVICE_CONTEXT context = MsklGetDeviceContext(Device);
    PDEVICE_OBJECT pdo = WdfDeviceWdmGetPhysicalDevice(Device);
    ULONG required = 0;
    DEVPROPTYPE propertyType = 0;
    GUID containerId;
    NTSTATUS status;

    status = IoGetDevicePropertyData(pdo, &DEVPKEY_Device_InstanceId,
        0, 0, sizeof(context->DeviceInstanceId), context->DeviceInstanceId,
        &required, &propertyType);
    if (!NT_SUCCESS(status) || propertyType != DEVPROP_TYPE_STRING) {
        return NT_SUCCESS(status) ? STATUS_OBJECT_TYPE_MISMATCH : status;
    }
    context->DeviceInstanceId[MSKL_MAX_INSTANCE_ID_CHARS - 1] = L'\0';

    status = IoGetDevicePropertyData(pdo, &DEVPKEY_Device_ContainerId,
        0, 0, sizeof(containerId), &containerId, &required,
        &propertyType);
    if (!NT_SUCCESS(status) || propertyType != DEVPROP_TYPE_GUID) {
        return NT_SUCCESS(status) ? STATUS_OBJECT_TYPE_MISMATCH : status;
    }
    return RtlStringCchPrintfW(context->ContainerId,
        MSKL_MAX_CONTAINER_ID_CHARS,
        L"%08X-%04X-%04X-%02X%02X-%02X%02X%02X%02X%02X%02X",
        (ULONG)containerId.Data1, (ULONG)containerId.Data2,
        (ULONG)containerId.Data3, (ULONG)containerId.Data4[0],
        (ULONG)containerId.Data4[1], (ULONG)containerId.Data4[2],
        (ULONG)containerId.Data4[3], (ULONG)containerId.Data4[4],
        (ULONG)containerId.Data4[5], (ULONG)containerId.Data4[6],
        (ULONG)containerId.Data4[7]);
}
