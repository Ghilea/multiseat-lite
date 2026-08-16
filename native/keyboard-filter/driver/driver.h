#pragma once

#include <ntddk.h>
#include <wdf.h>
#include <kbdmou.h>
#include <ntddkbd.h>
#include <devpkey.h>
#include <ntstrsafe.h>
#include "../shared/multiseat_keyboard_filter_protocol.h"

typedef struct _MSKL_DEVICE_CONTEXT {
    CONNECT_DATA UpperConnectData;
    BOOLEAN Connected;
    BOOLEAN TargetResolved;
    BOOLEAN ObservationEnabled;
    volatile LONG OpenServiceHandles;
    volatile LONG64 PacketsObserved;
    volatile LONG64 PassThroughPackets;
    volatile LONG64 ConnectRequests;
    volatile LONG64 PnpDisconnectEvents;
    volatile LONG64 BufferOverflowCount;
    volatile LONG64 NextSequence;
    volatile LONG LastNtStatus;
    volatile LONG64 LastHeartbeatInterruptTime;
    WCHAR DeviceInstanceId[MSKL_MAX_INSTANCE_ID_CHARS];
    WCHAR ContainerId[MSKL_MAX_CONTAINER_ID_CHARS];
    WCHAR TargetInstanceId[MSKL_MAX_INSTANCE_ID_CHARS];
    WDFSPINLOCK RingLock;
    ULONG RingHead;
    ULONG RingCount;
    MSKL_KEY_EVENT_COPY Ring[MSKL_RING_CAPACITY];
} MSKL_DEVICE_CONTEXT, *PMSKL_DEVICE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(MSKL_DEVICE_CONTEXT, MsklGetDeviceContext)

DRIVER_INITIALIZE DriverEntry;
EVT_WDF_DRIVER_DEVICE_ADD MsklEvtDeviceAdd;
EVT_WDF_DEVICE_RELEASE_HARDWARE MsklEvtDeviceReleaseHardware;
EVT_WDF_IO_QUEUE_IO_INTERNAL_DEVICE_CONTROL MsklEvtIoInternalDeviceControl;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL MsklEvtIoDeviceControl;
EVT_WDF_DEVICE_FILE_CREATE MsklEvtFileCreate;
EVT_WDF_FILE_CLEANUP MsklEvtFileCleanup;

VOID
MsklServiceCallback(
    _In_ PVOID NormalContext,
    _In_ PVOID SystemArgument1,
    _In_ PVOID SystemArgument2,
    _Inout_ PVOID SystemArgument3
    );
