# actr.toml — linked runtime configuration for the Kotlin Android client.
#
# Mirrors the Swift EchoApp: the client uses ActrNode.linked() and does NOT use
# `actr build`. This file is read directly as runtime configuration, so package,
# binary, and build sections are intentionally omitted. Package identity is
# declared in manifest.toml (read by resolveActorType()).
#
# The Android emulator reaches the host's localhost via 10.0.2.2.

[signaling]
url = "ws://__HOST__:__HTTP_PORT__/signaling/ws"

[issuer_endpoint]
url = "http://__HOST__:__HTTP_PORT__/issuer"

[deployment]
realm = "__REALM__"

[discovery]
visible = true

[observability]
filter_level = "info"
tracing_enabled = false

[webrtc]
# force_relay=true: the Android emulator can't do direct ICE to the host (QEMU
# NAT), so both peers relay through loran's TURN (enabled via the loran config
# enable bitmask, advertised at the host LAN IP __HOST__). TURN credentials are
# provisioned by actr from the registration response.
force_relay = true
stun_urls = ["stun:__HOST__:__ICE_PORT__"]
turn_urls = ["turn:__HOST__:__ICE_PORT__"]

[hyper]
data_dir = "__HYPER_DATA_DIR__"

[hyper.trust]
kind = "dev_only"

[acl]

[[acl.rules]]
permission = "allow"
type = "__ACL_TYPE__"
