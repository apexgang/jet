// Views over the generated client protocol models. The models are not
// decoders: validate a frame against jet-v1.schema.json, then read it here.

/// How this client labels a Plane before any Conversation is loaded.
func planeLabel(_ status: PlaneStatus) -> String {
    "\(status.plane_id) (jetd \(status.core_version))"
}

/// The negotiated protocol, or nil while the connection is unauthenticated.
func negotiatedProtocol(_ hello: ServerHello) -> String? {
    guard case let .welcome(welcome) = hello else { return nil }
    return "\(welcome.`protocol`).\(welcome.minor)"
}
