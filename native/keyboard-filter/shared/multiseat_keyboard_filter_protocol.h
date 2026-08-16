#pragma once

/* Fixed-width, versioned service/driver ABI. Friendly names and VID/PID are
 * deliberately absent: they are not authoritative identities. */
#if defined(_KERNEL_MODE)
/* Kernel projects must use the WDK integer definitions. Pulling the user-mode
 * CRT's stdint.h into a /kernel build conflicts with the WDK kernel CRT. */
typedef UINT8 uint8_t;
typedef UINT16 uint16_t;
typedef UINT32 uint32_t;
typedef UINT64 uint64_t;
#else
#include <stdint.h>
#endif

#define MSKL_PROTOCOL_MAGIC 0x4C4B534Du /* "MSKL" */
#define MSKL_PROTOCOL_VERSION 2u
#define MSKL_MAX_INSTANCE_ID_CHARS 512u
#define MSKL_MAX_CONTAINER_ID_CHARS 40u
#define MSKL_EVENT_BATCH_CAPACITY 64u
#define MSKL_RING_CAPACITY 256u
#define MSKL_DEVICE_INTERFACE_GUID_STRING L"{8423BD6E-CF40-4D1C-9FF5-489475802621}"

typedef enum MSKL_COMMAND_KIND {
    MsklCommandSetTargetDevice = 1,
    MsklCommandEnableSuppression = 2,
    MsklCommandDisableSuppression = 3,
    MsklCommandQueryStatus = 4,
    MsklCommandHeartbeat = 5,
    MsklCommandSetObservation = 6,
    MsklCommandReadEvents = 7
} MSKL_COMMAND_KIND;

typedef enum MSKL_SUPPRESSION_CAPABILITY {
    MsklSuppressionDisabledByBuild = 0
} MSKL_SUPPRESSION_CAPABILITY;

typedef enum MSKL_FAIL_OPEN_REASON {
    MsklFailOpenPrototypeLocked = 8
} MSKL_FAIL_OPEN_REASON;

typedef struct MSKL_MESSAGE_HEADER {
    uint32_t magic;
    uint16_t version;
    uint16_t size;
    uint32_t command;
    uint64_t request_id;
} MSKL_MESSAGE_HEADER, *PMSKL_MESSAGE_HEADER;

typedef struct MSKL_SET_TARGET_REQUEST {
    MSKL_MESSAGE_HEADER header;
    uint8_t physical_identity_digest[32];
    uint16_t collection_instance_id[MSKL_MAX_INSTANCE_ID_CHARS];
    uint16_t container_id[MSKL_MAX_CONTAINER_ID_CHARS];
} MSKL_SET_TARGET_REQUEST, *PMSKL_SET_TARGET_REQUEST;

typedef struct MSKL_OBSERVATION_REQUEST {
    MSKL_MESSAGE_HEADER header;
    uint8_t enabled;
    uint8_t reserved[7];
} MSKL_OBSERVATION_REQUEST, *PMSKL_OBSERVATION_REQUEST;

typedef struct MSKL_KEY_EVENT_COPY {
    uint64_t sequence;
    uint64_t interrupt_time_100ns;
    uint16_t unit_id;
    uint16_t make_code;
    uint16_t flags;
    uint16_t reserved;
} MSKL_KEY_EVENT_COPY, *PMSKL_KEY_EVENT_COPY;

typedef struct MSKL_EVENT_BATCH {
    MSKL_MESSAGE_HEADER header;
    uint32_t event_count;
    uint32_t reserved;
    MSKL_KEY_EVENT_COPY events[MSKL_EVENT_BATCH_CAPACITY];
} MSKL_EVENT_BATCH, *PMSKL_EVENT_BATCH;

typedef struct MSKL_STATUS_RESPONSE {
    MSKL_MESSAGE_HEADER header;
    uint8_t driver_loaded;
    uint8_t filter_attached;
    uint8_t target_resolved;
    uint8_t service_connected;
    uint8_t observation_enabled;
    uint8_t suppression_requested;
    uint8_t suppression_active;
    uint8_t lease_active;
    uint8_t fail_open;
    uint32_t suppression_capability;
    uint32_t fail_open_reason;
    uint32_t last_ntstatus;
    uint32_t ring_capacity;
    uint32_t ring_count;
    uint64_t packets_observed;
    uint64_t pass_through_packets;
    uint64_t connect_requests;
    uint64_t pnp_disconnect_events;
    uint64_t buffer_overflow_count;
    uint64_t last_heartbeat_interrupt_time_100ns;
    uint64_t lease_expires_interrupt_time_100ns;
    uint16_t device_instance_id[MSKL_MAX_INSTANCE_ID_CHARS];
    uint16_t container_id[MSKL_MAX_CONTAINER_ID_CHARS];
    uint16_t target_instance_id[MSKL_MAX_INSTANCE_ID_CHARS];
} MSKL_STATUS_RESPONSE, *PMSKL_STATUS_RESPONSE;

#if defined(_WIN32)
#if defined(_KERNEL_MODE)
#include <devioctl.h>
#else
#include <winioctl.h>
#endif
#define IOCTL_MSKL_SET_TARGET CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)
#define IOCTL_MSKL_ENABLE CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)
#define IOCTL_MSKL_DISABLE CTL_CODE(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)
#define IOCTL_MSKL_QUERY CTL_CODE(FILE_DEVICE_UNKNOWN, 0x803, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)
#define IOCTL_MSKL_HEARTBEAT CTL_CODE(FILE_DEVICE_UNKNOWN, 0x804, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)
#define IOCTL_MSKL_SET_OBSERVATION CTL_CODE(FILE_DEVICE_UNKNOWN, 0x805, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)
#define IOCTL_MSKL_READ_EVENTS CTL_CODE(FILE_DEVICE_UNKNOWN, 0x806, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)
#endif
