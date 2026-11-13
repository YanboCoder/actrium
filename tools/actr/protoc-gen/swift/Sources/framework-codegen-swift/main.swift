import CryptoKit
import Foundation
import SwiftProtobuf
import SwiftProtobufPluginLibrary

private enum DispatchRouteCodingKeys: String, CodingKey {
    case route_key, proto_file, proto_service, method, input_type, output_type
    case payload_type, reply_mode, dependency_alias
}

@main
struct ActrFrameworkGenerator {
    static let version = "0.11.0"

    // MARK: - DispatchSpec model

    struct DispatchRoute: Decodable {
        let route_key: String
        let proto_file: String
        let proto_service: String
        let method: String
        let input_type: String
        let output_type: String
        let payload_type: String
        let reply_mode: String
        let dependency_alias: String?

        var isRemote: Bool { dependency_alias != nil }

        init(
            route_key: String,
            proto_file: String,
            proto_service: String,
            method: String,
            input_type: String,
            output_type: String,
            payload_type: String,
            reply_mode: String = "return",
            dependency_alias: String? = nil
        ) {
            self.route_key = route_key
            self.proto_file = proto_file
            self.proto_service = proto_service
            self.method = method
            self.input_type = input_type
            self.output_type = output_type
            self.payload_type = payload_type
            self.reply_mode = reply_mode
            self.dependency_alias = dependency_alias
        }

        init(from decoder: Swift.Decoder) throws {
            let values = try decoder.container(keyedBy: DispatchRouteCodingKeys.self)
            route_key = try values.decode(String.self, forKey: .route_key)
            proto_file = try values.decode(String.self, forKey: .proto_file)
            proto_service = try values.decode(String.self, forKey: .proto_service)
            method = try values.decode(String.self, forKey: .method)
            input_type = try values.decode(String.self, forKey: .input_type)
            output_type = try values.decode(String.self, forKey: .output_type)
            payload_type = try values.decode(String.self, forKey: .payload_type)
            // reply_mode was added as an optional field to DispatchSpec v1.
            // Keep pre-RFC-0323 specs on the original return-byte contract.
            reply_mode = try values.decodeIfPresent(String.self, forKey: .reply_mode) ?? "return"
            dependency_alias = try values.decodeIfPresent(String.self, forKey: .dependency_alias)
        }
    }

    struct DispatchSpecModel: Decodable {
        let schema_version: String
        let routes: [DispatchRoute]
        let spec_hash: String?
    }

    // MARK: - Metadata model

    struct MethodMetadata: Encodable {
        let name: String
        let snake_name: String
        let route_key: String
        let input_ref: TypeRef
        let output_ref: TypeRef
        let reply_mode: String
    }

    struct LocalServiceMetadata: Encodable {
        let name: String
        let package: String
        let proto_file: String
        let handler_interface: String
        let methods: [MethodMetadata]
    }

    struct RemoteServiceMetadata: Encodable {
        let name: String
        let package: String
        let proto_file: String
        let dependency_alias: String
        let client_type: String
        let methods: [MethodMetadata]
    }

    struct ActrGenMetadata: Encodable {
        let schema_version: String
        let plugin_version: String
        let language: String
        let dispatch_spec_hash: String
        let generated_files: [String]
        let generated_local_routes: [String]
        let generated_remote_routes: [String]
        let local_services: [LocalServiceMetadata]
        let remote_services: [RemoteServiceMetadata]
    }

    // MARK: - Proto type resolution helpers

    struct TypeRef: Encodable {
        let proto_type: String
        let type_name: String
        let proto_package: String
        let proto_file: String
    }

    enum PluginError: LocalizedError {
        case missingDispatchSpec
        case missingDispatchSpecHash
        case cannotReadDispatchSpec(String)
        case hashMismatch(expected: String, actual: String)
        case invalidDispatchSpec(String)

        var errorDescription: String? {
            switch self {
            case .missingDispatchSpec:
                return "dispatch_spec parameter is required"
            case .missingDispatchSpecHash:
                return "dispatch_spec_hash parameter is required"
            case let .cannotReadDispatchSpec(detail):
                return "Cannot read dispatch spec: \(detail)"
            case let .hashMismatch(expected, actual):
                return "Dispatch spec hash mismatch: expected \(expected), got \(actual)"
            case let .invalidDispatchSpec(detail):
                return "Invalid dispatch spec: \(detail)"
            }
        }
    }

    // MARK: - Entry point

    static func main() throws {
        if CommandLine.arguments.contains("--version") || CommandLine.arguments.contains("-v") {
            print(version)
            return
        }

        let requestData = FileHandle.standardInput.readDataToEndOfFile()
        let request = try Google_Protobuf_Compiler_CodeGeneratorRequest(serializedBytes: requestData)

        let parameters = parseParameters(request.parameter)

        // Read and validate the dispatch spec.
        guard let specPath = parameters["dispatch_spec"] else {
            throw PluginError.missingDispatchSpec
        }
        guard let expectedHash = parameters["dispatch_spec_hash"] else {
            throw PluginError.missingDispatchSpecHash
        }
        let spec = try loadDispatchSpec(at: specPath, expectedHash: expectedHash)

        // Build proto-to-Swift type name map from the full descriptor set.
        var typeToSwiftName: [String: String] = [:]
        var typeRefs: [String: TypeRef] = [:]
        for fileDescriptor in request.protoFile {
            for message in fileDescriptor.messageType {
                collectSwiftTypeNames(
                    package: fileDescriptor.package,
                    message: message,
                    parentProtoName: nil,
                    parentSwiftName: nil,
                    into: &typeToSwiftName
                )
                collectTypeRefs(
                    package: fileDescriptor.package,
                    protoFile: normalizeProtoPath(fileDescriptor.name),
                    message: message,
                    parentProtoName: nil,
                    into: &typeRefs
                )
            }
        }

        // Group routes by proto_file for per-file generation.
        var routesByFile: [String: [DispatchRoute]] = [:]
        for route in spec.routes {
            guard route.reply_mode == "return" || route.reply_mode == "explicit" else {
                throw PluginError.invalidDispatchSpec(
                    "route `\(route.route_key)` has unsupported reply_mode `\(route.reply_mode)`"
                )
            }
            let normalizedFile = normalizeProtoPath(route.proto_file)
            routesByFile[normalizedFile, default: []].append(route)
        }

        // Collect all local and remote routes for the unified dispatcher.
        let localRoutes = spec.routes.filter { !$0.isRemote }
        let remoteRoutes = spec.routes.filter { $0.isRemote }
        let localServiceNames = Set(localRoutes.map { $0.proto_service })

        // Track generated file names for metadata.
        var generatedFileNames: [String] = []

        var response = Google_Protobuf_Compiler_CodeGeneratorResponse()

        // Per-file generation.
        for fileDescriptor in request.protoFile {
            guard request.fileToGenerate.contains(fileDescriptor.name) else { continue }

            let protoFile = normalizeProtoPath(fileDescriptor.name)
            guard let fileRoutes = routesByFile[protoFile], !fileRoutes.isEmpty else {
                // Skip files that have no dispatch routes.
                continue
            }

            let hasLocal = fileRoutes.contains { !$0.isRemote }

            var content = """
            // DO NOT EDIT.
            // Generated by protoc-gen-actrframework-swift

            import Foundation
            import SwiftProtobuf
            import Actr

            """

            // Group routes by proto_service for handler protocol / dispatcher generation.
            let routesByService = Dictionary(grouping: fileRoutes, by: { $0.proto_service })

            for (serviceFq, serviceRoutes) in routesByService.sorted(by: { $0.key < $1.key }) {
                let serviceName = serviceFq.split(separator: ".").last.map(String.init) ?? serviceFq
                let isLocalService = serviceRoutes.allSatisfy { !$0.isRemote }

                // 1. Per-RPC contract structs.
                for route in serviceRoutes.sorted(by: { $0.route_key < $1.route_key }) {
                    let inputSwift = resolveSwiftTypeName(route.input_type, typeToSwiftName: typeToSwiftName)
                    let outputSwift = resolveSwiftTypeName(route.output_type, typeToSwiftName: typeToSwiftName)
                    let structName = contractStructName(
                        protoService: serviceFq, method: route.method
                    )

                    content += """

                    public struct \(structName) {
                        public typealias Request = \(inputSwift)
                        public typealias Response = \(outputSwift)
                        public static var routeKey: String { "\(route.route_key)" }
                        public static var payloadType: PayloadType { \(try payloadTypeSwift(route.payload_type)) }

                        public static func encodeRequest(_ request: \(inputSwift)) throws -> Data {
                            try request.serializedData()
                        }

                        public static func decodeResponse(_ data: Data) throws -> \(outputSwift) {
                            try \(outputSwift)(serializedBytes: data)
                        }
                    }

                    """
                }

                // 2. Handler protocol (local only).
                if isLocalService {
                    let handlerProtocol = "\(serviceName)Handler"
                    content += """

                    public protocol \(handlerProtocol): Sendable {
                    """
                    for route in serviceRoutes.sorted(by: { $0.route_key < $1.route_key }) {
                        let methodName = lowerCamelCase(route.method)
                        let inputSwift = resolveSwiftTypeName(route.input_type, typeToSwiftName: typeToSwiftName)
                        let outputSwift = resolveSwiftTypeName(route.output_type, typeToSwiftName: typeToSwiftName)

                        if route.reply_mode == "explicit" {
                            content += """

                              func \(methodName)(
                                  req: \(inputSwift),
                                  ctx: any ActrContext,
                                  reply: Reply<\(outputSwift)>
                              ) async throws(ActrError)
                            """
                        } else {
                            content += """

                              func \(methodName)(
                                  req: \(inputSwift),
                                  ctx: any ActrContext
                              ) async throws(ActrError) -> \(outputSwift)
                            """
                        }
                    }
                    content += "\n}\n"
                }
            }

            let suffix = hasLocal ? ".actor.swift" : ".client.swift"
            var generatedFile = Google_Protobuf_Compiler_CodeGeneratorResponse.File()
            generatedFile.name = fileDescriptor.name.replacingOccurrences(of: ".proto", with: suffix)
            generatedFile.content = content
            response.file.append(generatedFile)
            generatedFileNames.append(generatedFile.name)
        }

        // Generate unified dispatcher: actr_dispatch.swift
        let dispatchContent = try generateUnifiedDispatcher(
            localRoutes: localRoutes,
            remoteRoutes: remoteRoutes,
            localServiceNames: localServiceNames,
            typeToSwiftName: typeToSwiftName
        )

        var dispatchFile = Google_Protobuf_Compiler_CodeGeneratorResponse.File()
        dispatchFile.name = "actr_dispatch.swift"
        dispatchFile.content = dispatchContent
        response.file.append(dispatchFile)
        generatedFileNames.append("actr_dispatch.swift")

        // Generate metadata.
        generatedFileNames.append("actr-gen-meta.json")
        let metadata = ActrGenMetadata(
            schema_version: spec.schema_version,
            plugin_version: version,
            language: "swift",
            dispatch_spec_hash: expectedHash,
            generated_files: generatedFileNames,
            generated_local_routes: localRoutes.map { $0.route_key }.sorted(),
            generated_remote_routes: remoteRoutes.map { $0.route_key }.sorted(),
            local_services: buildLocalServiceMetadata(localRoutes, typeRefs: typeRefs),
            remote_services: buildRemoteServiceMetadata(remoteRoutes, typeRefs: typeRefs)
        )

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        var metadataFile = Google_Protobuf_Compiler_CodeGeneratorResponse.File()
        metadataFile.name = "actr-gen-meta.json"
        metadataFile.content = String(bytes: try encoder.encode(metadata), encoding: .utf8) ?? "{}"
        response.file.append(metadataFile)

        try FileHandle.standardOutput.write(response.serializedData())
    }

    // MARK: - Metadata generation

    static func buildLocalServiceMetadata(
        _ routes: [DispatchRoute],
        typeRefs: [String: TypeRef]
    ) -> [LocalServiceMetadata] {
        Dictionary(grouping: routes, by: { $0.proto_service })
            .sorted { $0.key < $1.key }
            .map { protoService, serviceRoutes in
                let (packageName, serviceName) = splitProtoService(protoService)
                return LocalServiceMetadata(
                    name: serviceName,
                    package: packageName,
                    proto_file: serviceRoutes[0].proto_file,
                    handler_interface: "\(serviceName)Handler",
                    methods: serviceRoutes
                        .sorted { $0.route_key < $1.route_key }
                        .map { methodMetadata($0, typeRefs: typeRefs) }
                )
            }
    }

    static func buildRemoteServiceMetadata(
        _ routes: [DispatchRoute],
        typeRefs: [String: TypeRef]
    ) -> [RemoteServiceMetadata] {
        Dictionary(grouping: routes, by: { $0.proto_service })
            .sorted { $0.key < $1.key }
            .map { protoService, serviceRoutes in
                let (packageName, serviceName) = splitProtoService(protoService)
                return RemoteServiceMetadata(
                    name: serviceName,
                    package: packageName,
                    proto_file: serviceRoutes[0].proto_file,
                    dependency_alias: serviceRoutes[0].dependency_alias ?? "",
                    client_type: "\(serviceName)Client",
                    methods: serviceRoutes
                        .sorted { $0.route_key < $1.route_key }
                        .map { methodMetadata($0, typeRefs: typeRefs) }
                )
            }
    }

    static func methodMetadata(
        _ route: DispatchRoute,
        typeRefs: [String: TypeRef]
    ) -> MethodMetadata {
        MethodMetadata(
            name: route.method,
            snake_name: snakeCase(route.method),
            route_key: route.route_key,
            input_ref: metadataTypeRef(
                route.input_type,
                fallbackProtoFile: route.proto_file,
                typeRefs: typeRefs
            ),
            output_ref: metadataTypeRef(
                route.output_type,
                fallbackProtoFile: route.proto_file,
                typeRefs: typeRefs
            ),
            reply_mode: route.reply_mode
        )
    }

    static func metadataTypeRef(
        _ rawType: String,
        fallbackProtoFile: String,
        typeRefs: [String: TypeRef]
    ) -> TypeRef {
        let protoType = String(rawType.drop(while: { $0 == "." }))
        if let typeRef = typeRefs[protoType] {
            return typeRef
        }

        let components = protoType.split(separator: ".").map(String.init)
        return TypeRef(
            proto_type: protoType,
            type_name: components.last ?? protoType,
            proto_package: components.dropLast().joined(separator: "."),
            proto_file: fallbackProtoFile
        )
    }

    static func splitProtoService(_ protoService: String) -> (String, String) {
        guard let separator = protoService.lastIndex(of: ".") else {
            return ("", protoService)
        }
        return (
            String(protoService[..<separator]),
            String(protoService[protoService.index(after: separator)...])
        )
    }

    // MARK: - Unified dispatcher generation

    static func generateUnifiedDispatcher(
        localRoutes: [DispatchRoute],
        remoteRoutes: [DispatchRoute],
        localServiceNames: Set<String>,
        typeToSwiftName: [String: String]
    ) throws -> String {
        var content = """
        // DO NOT EDIT.
        // Generated by protoc-gen-actrframework-swift

        import Foundation
        import SwiftProtobuf
        import Actr

        """

        // ActrDispatchHandler protocol — aggregates all local handler protocols.
        let handlerProtocols = localServiceNames
            .map { "\($0.split(separator: ".").last.map(String.init) ?? $0)Handler" }
            .sorted()
            .joined(separator: ", ")

        if !localServiceNames.isEmpty {
            content += """

            public protocol ActrDispatchHandler: \(handlerProtocols), Sendable {}

            """
        } else {
            content += """

            public protocol ActrDispatchHandler: Sendable {}

            public struct EmptyActrDispatchHandler: ActrDispatchHandler {
                public init() {}
            }

            """
        }

        // actrDispatch function.
        content += """

        public func actrDispatch(
            ctx: any ActrContext,
            handler: any ActrDispatchHandler,
            routeKey: String,
            payload: Data,
            timeoutMs: Int64
        ) async throws(ActrError) -> Data {
            switch routeKey {
        """

        // Local routes: decode → invoke → encode.
        for route in localRoutes.sorted(by: { $0.route_key < $1.route_key }) {
            let methodName = lowerCamelCase(route.method)
            let inputSwift = resolveSwiftTypeName(route.input_type, typeToSwiftName: typeToSwiftName)
            let outputSwift = resolveSwiftTypeName(route.output_type, typeToSwiftName: typeToSwiftName)

            content += """

              case \(swiftStringLiteral(route.route_key)):
                  let req: \(inputSwift)
                  do {
                      req = try \(inputSwift)(serializedBytes: payload)
                  } catch {
                      throw ActrError.DecodeFailure(msg: "Failed to decode \(inputSwift) for route \\(routeKey): \\(error)")
                  }
            """
            content += "\n"
            if route.reply_mode == "explicit" {
                content += """
                  try await ctx.claimReply()
                  let reply = Reply<\(outputSwift)>(ctx: ctx) { response in
                      do {
                          return try response.serializedData()
                      } catch {
                          throw ActrError.DecodeFailure(msg: "Failed to encode \(outputSwift) for route \\(routeKey): \\(error)")
                      }
                  }
                  try await handler.\(methodName)(req: req, ctx: ctx, reply: reply)
                  return Data()
                """
            } else {
                content += """
                  let resp = try await handler.\(methodName)(req: req, ctx: ctx)
                  let encoded: Data
                  do {
                      encoded = try resp.serializedData()
                  } catch {
                      throw ActrError.DecodeFailure(msg: "Failed to encode \(outputSwift) for route \\(routeKey): \\(error)")
                  }
                  do {
                      try await ctx.replyRaw(payload: encoded)
                      return Data()
                  } catch {
                      if Task.isCancelled {
                          throw error
                      }
                      return encoded
                  }
                """
            }
        }

        // Remote routes: resolve dependency → discover → call.
        for route in remoteRoutes.sorted(by: { $0.route_key < $1.route_key }) {
            let alias = route.dependency_alias ?? ""

            content += """

              case \(swiftStringLiteral(route.route_key)):
                  let actrType = try await ctx.resolveDependency(alias: \(swiftStringLiteral(alias)))
                  let target = try await ctx.discover(targetType: actrType)
                  let response = try await ctx.callRaw(
                      target: target,
                      routeKey: routeKey,
                      payloadType: \(try payloadTypeSwift(route.payload_type)),
                      payload: payload,
                      timeoutMs: timeoutMs
                  )
                  do {
                      try await ctx.replyRaw(payload: response)
                      return Data()
                  } catch {
                      if Task.isCancelled {
                          throw error
                      }
                      return response
                  }
            """
        }

        content += """

            default:
                throw ActrError.UnknownRoute(msg: "Unknown route: \\(routeKey)")
            }
        }
        """

        return content
    }

    // MARK: - DispatchSpec loading and hash validation

    static func loadDispatchSpec(at path: String, expectedHash: String) throws -> DispatchSpecModel {
        let url = URL(fileURLWithPath: path)
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            throw PluginError.cannotReadDispatchSpec(error.localizedDescription)
        }

        try validateDispatchSpecJSONShape(data: data)

        let spec: DispatchSpecModel
        do {
            spec = try JSONDecoder().decode(DispatchSpecModel.self, from: data)
        } catch {
            throw PluginError.cannotReadDispatchSpec(error.localizedDescription)
        }
        try validateDispatchSpec(spec)

        // Validate hash: recompute JCS+SHA-256 over the spec without spec_hash.
        let actualHash = try computeDispatchSpecHash(data: data)
        if let embeddedHash = spec.spec_hash, embeddedHash != actualHash {
            throw PluginError.hashMismatch(expected: embeddedHash, actual: actualHash)
        }
        guard actualHash == expectedHash else {
            throw PluginError.hashMismatch(expected: expectedHash, actual: actualHash)
        }

        return spec
    }

    /// Reject fields and value shapes outside DispatchSpec v1 before
    /// `JSONDecoder` drops unknown keys from the typed model.
    static func validateDispatchSpecJSONShape(data: Data) throws {
        let json: Any
        do {
            json = try JSONSerialization.jsonObject(with: data)
        } catch {
            throw PluginError.cannotReadDispatchSpec(error.localizedDescription)
        }
        guard let object = json as? [String: Any] else {
            throw PluginError.invalidDispatchSpec("dispatch spec must be a JSON object")
        }

        let allowedSpecFields: Set<String> = ["schema_version", "routes", "spec_hash"]
        let unknownSpecFields = Set(object.keys).subtracting(allowedSpecFields)
        guard unknownSpecFields.isEmpty else {
            throw PluginError.invalidDispatchSpec(
                "unknown top-level field(s): \(unknownSpecFields.sorted().joined(separator: ", "))"
            )
        }
        guard object["schema_version"] is String, let routes = object["routes"] as? [Any] else {
            throw PluginError.invalidDispatchSpec(
                "schema_version must be a string and routes must be an array"
            )
        }
        if let hash = object["spec_hash"], !(hash is String), !(hash is NSNull) {
            throw PluginError.invalidDispatchSpec("spec_hash must be a string when present")
        }

        let requiredRouteFields: Set<String> = [
            "route_key", "proto_file", "proto_service", "method",
            "input_type", "output_type", "payload_type",
        ]
        let allowedRouteFields = requiredRouteFields.union(["dependency_alias", "reply_mode"])
        for (index, value) in routes.enumerated() {
            guard let route = value as? [String: Any] else {
                throw PluginError.invalidDispatchSpec("route #\(index) must be a JSON object")
            }
            let routeFields = Set(route.keys)
            let missing = requiredRouteFields.subtracting(routeFields)
            let unknown = routeFields.subtracting(allowedRouteFields)
            guard missing.isEmpty else {
                throw PluginError.invalidDispatchSpec(
                    "route #\(index) is missing field(s): \(missing.sorted().joined(separator: ", "))"
                )
            }
            guard unknown.isEmpty else {
                throw PluginError.invalidDispatchSpec(
                    "route #\(index) contains unknown field(s): \(unknown.sorted().joined(separator: ", "))"
                )
            }
            for field in requiredRouteFields {
                guard let string = route[field] as? String, !string.isEmpty else {
                    throw PluginError.invalidDispatchSpec(
                        "route #\(index) field `\(field)` must be a non-empty string"
                    )
                }
            }
            let replyMode = (route["reply_mode"] as? String) ?? "return"
            guard replyMode == "return" || replyMode == "explicit" else {
                throw PluginError.invalidDispatchSpec(
                    "route #\(index) has unsupported reply_mode"
                )
            }
            if let alias = route["dependency_alias"], !(alias is String), !(alias is NSNull) {
                throw PluginError.invalidDispatchSpec(
                    "route #\(index) dependency_alias must be a string when present"
                )
            }
        }
    }

    static func validateDispatchSpec(_ spec: DispatchSpecModel) throws {
        guard spec.schema_version == "v1" else {
            throw PluginError.invalidDispatchSpec(
                "unsupported schema_version `\(spec.schema_version)`; expected `v1`"
            )
        }

        var routeKeys = Set<String>()
        var aliasesByService: [String: Set<String?>] = [:]
        for route in spec.routes {
            guard routeKeys.insert(route.route_key).inserted else {
                throw PluginError.invalidDispatchSpec(
                    "duplicate route_key `\(route.route_key)`"
                )
            }
            if let alias = route.dependency_alias,
               alias.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
                throw PluginError.invalidDispatchSpec(
                    "remote route `\(route.route_key)` has an empty dependency_alias"
                )
            }
            aliasesByService[route.proto_service, default: []].insert(route.dependency_alias)
        }
        for (service, aliases) in aliasesByService where aliases.count != 1 {
            throw PluginError.invalidDispatchSpec(
                "service `\(service)` mixes local/remote routes or dependency aliases"
            )
        }
    }

    /// Recompute the canonical JSON hash (JCS + SHA-256) over the spec
    /// **without** the `spec_hash` field.
    static func computeDispatchSpecHash(data: Data) throws -> String {
        // Decode as a generic JSON object so we can strip spec_hash.
        let jsonObject: Any
        do {
            jsonObject = try JSONSerialization.jsonObject(with: data)
        } catch {
            throw PluginError.cannotReadDispatchSpec(error.localizedDescription)
        }

        // Remove spec_hash from the top-level object.
        if let dict = jsonObject as? [String: Any] {
            var mutable = dict
            mutable.removeValue(forKey: "spec_hash")
            let canonical = try canonicalizeJSONObject(mutable)
            let hash = SHA256.hash(data: canonical)
            return hash.map { String(format: "%02x", $0) }.joined()
        }

        throw PluginError.cannotReadDispatchSpec("spec is not a JSON object")
    }

    /// Canonicalize a JSON object per JCS (RFC 8785): recursively sorted keys,
    /// no whitespace, UTF-8.
    static func canonicalizeJSONObject(_ value: Any) throws -> Data {
        Data(try canonicalJSONString(value).utf8)
    }

    /// Encode the string-only DispatchSpec shape using RFC 8785 JSON
    /// canonicalization. DispatchSpec v1 contains no numeric values.
    static func canonicalJSONString(_ value: Any) throws -> String {
        if let dict = value as? [String: Any] {
            let fields = try dict.keys.sorted().map { key -> String in
                guard let child = dict[key] else {
                    throw PluginError.invalidDispatchSpec(
                        "DispatchSpec object key disappeared during canonicalization"
                    )
                }
                return "\(jcsStringLiteral(key)):\(try canonicalJSONString(child))"
            }
            return "{\(fields.joined(separator: ","))}"
        }
        if let arr = value as? [Any] {
            let elements = try arr.map { try canonicalJSONString($0) }
            return "[\(elements.joined(separator: ","))]"
        }
        if let string = value as? String {
            return jcsStringLiteral(string)
        }
        if value is NSNull {
            return "null"
        }
        throw PluginError.invalidDispatchSpec(
            "DispatchSpec v1 canonicalization encountered a non-string primitive"
        )
    }

    static func jcsStringLiteral(_ value: String) -> String {
        var literal = "\""
        for scalar in value.unicodeScalars {
            switch scalar.value {
            case 0x08: literal += "\\b"
            case 0x09: literal += "\\t"
            case 0x0A: literal += "\\n"
            case 0x0C: literal += "\\f"
            case 0x0D: literal += "\\r"
            case 0x22: literal += "\\\""
            case 0x5C: literal += "\\\\"
            case 0x00 ... 0x1F:
                literal += String(format: "\\u%04x", scalar.value)
            default:
                literal.append(contentsOf: String(scalar))
            }
        }
        literal += "\""
        return literal
    }

    // MARK: - Type name resolution

    /// Resolve a fully-qualified proto type (e.g. `.echo.EchoRequest`) to its
    /// generated Swift type name using the declaring package's prefix.
    static func resolveSwiftTypeName(
        _ rawType: String,
        typeToSwiftName: [String: String]
    ) -> String {
        let trimmed = String(rawType.drop(while: { $0 == "." }))
        if let swiftName = typeToSwiftName[trimmed] {
            return swiftName
        }
        // Fallback: use the last component with no package prefix.
        let typeName = trimmed.split(separator: ".").last.map(String.init) ?? trimmed
        return typeName
    }

    static func collectSwiftTypeNames(
        package: String,
        message: Google_Protobuf_DescriptorProto,
        parentProtoName: String?,
        parentSwiftName: String?,
        into typeToSwiftName: inout [String: String]
    ) {
        let protoName = parentProtoName.map { "\($0).\(message.name)" } ?? message.name
        let fullProtoName = package.isEmpty ? protoName : "\(package).\(protoName)"
        let swiftName =
            parentSwiftName.map { "\($0).\(message.name)" }
                ?? "\(swiftPackagePrefix(package))\(message.name)"
        typeToSwiftName[fullProtoName] = swiftName

        for nested in message.nestedType {
            collectSwiftTypeNames(
                package: package,
                message: nested,
                parentProtoName: protoName,
                parentSwiftName: swiftName,
                into: &typeToSwiftName
            )
        }
    }

    static func collectTypeRefs(
        package: String,
        protoFile: String,
        message: Google_Protobuf_DescriptorProto,
        parentProtoName: String?,
        into typeRefs: inout [String: TypeRef]
    ) {
        let protoName = parentProtoName.map { "\($0).\(message.name)" } ?? message.name
        let fullProtoName = package.isEmpty ? protoName : "\(package).\(protoName)"
        typeRefs[fullProtoName] = TypeRef(
            proto_type: fullProtoName,
            type_name: protoName,
            proto_package: package,
            proto_file: protoFile
        )

        for nested in message.nestedType {
            collectTypeRefs(
                package: package,
                protoFile: protoFile,
                message: nested,
                parentProtoName: protoName,
                into: &typeRefs
            )
        }
    }

    // MARK: - Naming helpers

    /// Contract struct name: `<Package><Service><Method>Rpc`.
    /// Example: `echo.EchoService.Echo` → `EchoEchoServiceEchoRpc`.
    static func contractStructName(protoService: String, method: String) -> String {
        let parts = protoService.split(separator: ".")
        let packagePart = parts.dropLast().map { pascalCase(String($0)) }.joined()
        let servicePart = parts.last.map(String.init) ?? ""
        return "\(packagePart)\(servicePart)\(pascalCase(method))Rpc"
    }

    static func pascalCase(_ value: String) -> String {
        let parts = value.split(separator: "_")
        return parts.map { part -> String in
            guard let first = part.first else { return "" }
            return String(first).uppercased() + String(part.dropFirst())
        }.joined()
    }

    static func swiftPackagePrefix(_ packageName: String) -> String {
        if packageName.isEmpty { return "" }
        let components = packageName.split(separator: ".").map { component in
            component.split(separator: "_").map { $0.capitalized }.joined()
        }
        return components.joined(separator: "_") + "_"
    }

    static func snakeCase(_ value: String) -> String {
        let acronymBoundaries = value.replacingOccurrences(
            of: #"(.)([A-Z][a-z]+)"#,
            with: "$1_$2",
            options: .regularExpression
        )
        return acronymBoundaries.replacingOccurrences(
            of: #"([a-z0-9])([A-Z])"#,
            with: "$1_$2",
            options: .regularExpression
        ).lowercased()
    }

    static func lowerCamelCase(_ value: String) -> String {
        let parts = snakeCase(value).split(separator: "_")
        guard let first = parts.first else { return "" }
        return String(first) + parts.dropFirst().map { $0.capitalized }.joined()
    }

    static func normalizeProtoPath(_ value: String) -> String {
        var path = value.replacingOccurrences(of: "\\", with: "/")
        while path.hasPrefix("./") {
            path.removeFirst(2)
        }
        return path.hasSuffix(".proto") ? path : "\(path).proto"
    }

    static func swiftStringLiteral(_ value: String) -> String {
        var literal = "\""
        for scalar in value.unicodeScalars {
            switch scalar.value {
            case 0x22: literal += "\\\""
            case 0x5C: literal += "\\\\"
            case 0x0A: literal += "\\n"
            case 0x0D: literal += "\\r"
            case 0x09: literal += "\\t"
            case 0x00 ... 0x1F:
                literal += String(format: "\\u{%X}", scalar.value)
            case 0x2028, 0x2029:
                literal += String(format: "\\u{%X}", scalar.value)
            default:
                literal.append(contentsOf: String(scalar))
            }
        }
        literal += "\""
        return literal
    }

    static func payloadTypeSwift(_ value: String) throws -> String {
        switch value.lowercased() {
        case "rpcreliable": return ".rpcReliable"
        case "rpcsignal": return ".rpcSignal"
        case "streamreliable": return ".streamReliable"
        case "streamlatencyfirst": return ".streamLatencyFirst"
        case "mediartp": return ".mediaRtp"
        default:
            throw PluginError.invalidDispatchSpec("Unsupported payload_type `\(value)` in DispatchSpec route")
        }
    }

    // MARK: - Parameter parsing

    static func parseParameters(_ parameter: String) -> [String: String] {
        parameter.split(separator: ",").reduce(into: [String: String]()) { dict, pair in
            let parts = pair.split(separator: "=", maxSplits: 1)
            if parts.count == 2 {
                dict[String(parts[0])] = String(parts[1])
            } else {
                dict[String(parts[0])] = ""
            }
        }
    }
}
