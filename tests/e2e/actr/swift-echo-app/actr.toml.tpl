# actr.toml - EchoApp linked runtime configuration
#
# EchoApp uses linked mode through ActrNode.linked() and does not use
# `actr build`. This file is read directly as runtime configuration, so
# package, binary, and build sections are intentionally omitted.
#
# The actor type is defined in Swift code:
#     ActrType(publisher: "e2e.actrium.example", name: "EchoApp", version: "0.1.0")

[signaling]
url = "ws://__HOST__:__HTTP_PORT__/signaling/ws"

[issuer_endpoint]
url = "http://__HOST__:__HTTP_PORT__/issuer"

[deployment]
# Replace this with the Realm name created via the loran Admin UI.
realm = "__REALM__"

[discovery]
visible = true

[observability]
filter_level = "info"
tracing_enabled = true
tracing_endpoint = "http://localhost:4317"
tracing_service_name = "echo-app-ios"

[webrtc]
force_relay = false
stun_urls = ["stun:__HOST__:__ICE_PORT__"]
turn_urls = ["turn:__HOST__:__ICE_PORT__"]

[acl]

[[acl.rules]]
permission = "allow"
type = "e2e.actrium.example/EchoService@1.0.0"
