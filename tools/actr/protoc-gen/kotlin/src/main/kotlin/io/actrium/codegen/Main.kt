/**
 * ACTR Framework Protoc Plugin for Kotlin
 *
 * This is a protoc plugin that generates Actor framework code from proto service definitions. It
 * generates:
 * - Handler interface for service implementations
 * - RPC contracts for generated service types
 * - Unified `actr_dispatch.kt` with exact route-key matching sourced from DispatchSpec v1
 *
 * Usage as protoc plugin: protoc --plugin=protoc-gen-actrframework-kotlin=PATH \
 * ```
 *          --actrframework-kotlin_out=OUT_DIR input.proto
 * ```
 */
package io.actrium.codegen

import com.google.protobuf.DescriptorProtos.DescriptorProto
import com.google.protobuf.DescriptorProtos.FileDescriptorProto
import com.google.protobuf.compiler.PluginProtos.CodeGeneratorRequest
import com.google.protobuf.compiler.PluginProtos.CodeGeneratorResponse
import org.json.JSONArray
import org.json.JSONObject

fun main(args: Array<String>) {
    // Support --version and --help arguments
    if (args.isNotEmpty()) {
        when (args[0]) {
            "--version", "-V" -> {
                println("protoc-gen-actrframework-kotlin 0.11.0")
                return
            }
            "--help", "-h" -> {
                println(
                        "protoc-gen-actrframework-kotlin - Protobuf plugin for Actrium Kotlin framework"
                )
                println()
                println("USAGE:")
                println(
                        "    As protoc plugin: protoc --plugin=protoc-gen-actrframework-kotlin=PATH --actrframework-kotlin_out=OUT_DIR input.proto"
                )
                println("    Version info:     protoc-gen-actrframework-kotlin --version")
                println()
                println("VERSION:")
                println("    0.11.0")
                return
            }
        }
    }

    // Read CodeGeneratorRequest from stdin
    val request = CodeGeneratorRequest.parseFrom(System.`in`)

    // Generate code
    val response = generateCode(request)

    // Write CodeGeneratorResponse to stdout
    response.writeTo(System.out)
}

/** Generate code from the protoc request */
fun generateCode(request: CodeGeneratorRequest): CodeGeneratorResponse {
    val responseBuilder = CodeGeneratorResponse.newBuilder()

    // Parse parameters
    val params = parseParameters(request.parameter)
    val kotlinPackage = params["kotlin_package"] ?: "com.example.generated"

    // Load DispatchSpec (phase 6 unified-dispatch model)
    val dispatchSpecPath = params["dispatch_spec"]
    val dispatchSpecHash = params["dispatch_spec_hash"] ?: ""
    val dispatchSpec = if (dispatchSpecPath != null) {
        val spec = DispatchSpec.fromPath(dispatchSpecPath)
        spec.verifyHash(dispatchSpecHash)
        spec
    } else {
        throw IllegalArgumentException(
            "dispatch_spec parameter is required; pass --actr_opt=dispatch_spec=<path>"
        )
    }

    // Build fully-qualified message name -> declaring (package, proto file) map
    // from the full descriptor set so imported RPC message types resolve to
    // their real owner outer class instead of the current service's package.
    val typeOwner = mutableMapOf<String, TypeOwner>()
    for (file in request.protoFileList) {
        for (message in file.messageTypeList) {
            collectTypeOwners(file, message, emptyList(), typeOwner)
        }
    }

    // Track generated files, local routes, and remote routes for metadata.
    val generatedFiles = mutableListOf<String>()
    // Process each file to generate per-service handler/contract files.
    for (fileName in request.fileToGenerateList) {
        val fileDescriptor = request.protoFileList.find { it.name == fileName } ?: continue
        val protoFileName = fileName.substringAfterLast("/")

        // Process each service in the file
        for (service in fileDescriptor.serviceList) {
            val generator =
                    KotlinActorGenerator(
                            packageName = fileDescriptor.`package`,
                            serviceName = service.name,
                            methods = service.methodList,
                            params = params,
                            protoFileName = protoFileName,
                            typeOwner = typeOwner
                    )

            val generatedFile = generator.generate()
            responseBuilder.addFile(generatedFile)
            generatedFiles.add(generatedFile.name)

        }
    }

    // Generate the unified `actr_dispatch.kt` with exact route-key matching.
    val dispatchCode =
            generateActrDispatch(
                    dispatchSpec,
                    kotlinPackage,
                    typeOwner,
            )
    val dispatchFile =
            CodeGeneratorResponse.File.newBuilder()
                    .setName("actr_dispatch.kt")
                    .setContent(dispatchCode)
                    .build()
    responseBuilder.addFile(dispatchFile)
    generatedFiles.add("actr_dispatch.kt")

    // Generate `actr-gen-meta.json` with the new unified-dispatch fields.
    responseBuilder.addFile(
            CodeGeneratorResponse.File.newBuilder()
                    .setName("actr-gen-meta.json")
                    .setContent(
                            buildMetadataJson(
                                    dispatchSpec,
                                    dispatchSpecHash,
                                    generatedFiles,
                                    typeOwner,
                            ))
                    .build())

    return responseBuilder.build()
}

/** Generate the unified `actr_dispatch.kt` with exact route-key matching. */
private fun generateActrDispatch(
        spec: DispatchSpec,
        kotlinPackage: String,
        typeOwner: Map<String, TypeOwner>,
): String {
    val localRoutes = spec.localRoutes()
    val remoteRoutes = spec.remoteRoutes()

    // Build handler interface aggregating all local service handlers.
    val localServiceNames = localRoutes.map { it.protoService }.distinct().sorted()
    val hasLocal = localRoutes.isNotEmpty()

    val handlerInterfaces =
            if (hasLocal) {
                localServiceNames.joinToString(", ") { "${extractServiceName(it)}Handler" }
            } else {
                ""
            }

    // Build imports for protobuf message types used in local route decode/encode.
    val localImports = mutableSetOf<String>()
    for (route in localRoutes) {
        val inputOwner = typeOwner[route.inputType.trimStart('.')]
        if (inputOwner != null) {
            localImports.add(buildImport(inputOwner))
        }
        val outputOwner = typeOwner[route.outputType.trimStart('.')]
        if (outputOwner != null) {
            localImports.add(buildImport(outputOwner))
        }
    }

    val importsStr = if (localImports.isNotEmpty()) {
        localImports.filter { it.isNotEmpty() }.sorted().joinToString("\n") { "import $it.*" }
    } else {
        ""
    }

    // Build the `when` cases for exact route-key matching.
    val cases = StringBuilder()
    for (route in localRoutes) {
        val inputType = resolveKotlinType(route.inputType, typeOwner)
        val handlerMethod = route.method.toSnakeCase()

        cases.appendLine("    ${kotlinStringLiteral(route.routeKey)} -> {")
        cases.appendLine("        val request = $inputType.parseFrom(payload)")
        if (route.replyMode == "explicit") {
            val outputType = resolveKotlinType(route.outputType, typeOwner)
            cases.appendLine("        ctx.claimReply()")
            cases.appendLine(
                    "        val reply = Reply(ctx) { response: $outputType -> response.toByteArray() }"
            )
            cases.appendLine("        handler.${handlerMethod}(request, ctx, reply)")
            cases.appendLine("        ByteArray(0)")
        } else {
            cases.appendLine("        val response = handler.${handlerMethod}(request, ctx)")
            cases.appendLine("        val responseBytes = response.toByteArray()")
            cases.appendLine("        try {")
            cases.appendLine("            ctx.replyRaw(responseBytes)")
            cases.appendLine("            ByteArray(0)")
            cases.appendLine("        } catch (error: Exception) {")
            cases.appendLine("            if (error is kotlinx.coroutines.CancellationException) throw error")
            cases.appendLine("            responseBytes")
            cases.appendLine("        }")
        }
        cases.appendLine("    }")
    }
    for (route in remoteRoutes) {
        val alias = route.dependencyAlias ?: continue
        val payloadType = payloadTypeKotlin(route.payloadType)
        cases.appendLine("    ${kotlinStringLiteral(route.routeKey)} -> {")
        cases.appendLine("        require(timeoutMs > 0) { \"timeoutMs must be > 0, got \$timeoutMs\" }")
        cases.appendLine("        val actrType = ctx.resolveDependency(${kotlinStringLiteral(alias)})")
        cases.appendLine("        val target = ctx.discover(actrType)")
        cases.appendLine("        val response = ctx.callRaw(target, routeKey, $payloadType, payload, timeoutMs)")
        cases.appendLine("        try {")
        cases.appendLine("            ctx.replyRaw(response)")
        cases.appendLine("            ByteArray(0)")
        cases.appendLine("        } catch (error: Exception) {")
        cases.appendLine("            if (error is kotlinx.coroutines.CancellationException) throw error")
        cases.appendLine("            response")
        cases.appendLine("        }")
        cases.appendLine("    }")
    }

    val elseCase =
            "    else -> throw io.actrium.actr.ActrException.UnknownRoute(\"Unknown route: \$routeKey\")"

    val handlerInterface =
            if (hasLocal) {
                """
                |/**
                | * Unified handler interface aggregating all local service handlers.
                | */
                |interface ActrDispatchHandler : $handlerInterfaces {
                |    // All methods are inherited from individual service handlers.
                |}
                """.trimMargin()
            } else {
                """
                |/** Marker handler for a DispatchSpec without local routes. */
                |interface ActrDispatchHandler
                |
                |object EmptyActrDispatchHandler : ActrDispatchHandler
                """.trimMargin()
            }

    return buildString {
        appendLine("/**")
        appendLine(" * Auto-generated unified dispatcher - DO NOT EDIT.")
        appendLine(" *")
        appendLine(" * Generated by protoc-gen-actrframework-kotlin from DispatchSpec v1.")
        appendLine(" * Exact route-key matching (no prefix matching).")
        appendLine(" *")
        appendLine(" * Local routes: ${localRoutes.size}")
        appendLine(" * Remote routes: ${remoteRoutes.size}")
        appendLine(" */")
        appendLine("package $kotlinPackage")
        appendLine()
        appendLine("import io.actrium.actr.dsl.ActrContext")
        if (localRoutes.any { it.replyMode == "explicit" }) {
            appendLine("import io.actrium.actr.dsl.Reply")
        }
        if (remoteRoutes.isNotEmpty()) {
            appendLine("import io.actrium.actr.PayloadType")
        }
        if (importsStr.isNotEmpty()) {
            appendLine(importsStr)
        }
        if (hasLocal) {
            appendLine()
            appendLine("// Import individual service handler contracts.")
            for (name in localServiceNames) {
                val svc = extractServiceName(name)
                appendLine("import $kotlinPackage.${svc}Handler")
            }
        }
        appendLine()
        appendLine(handlerInterface)
        appendLine()
        appendLine("/**")
        appendLine(" * Unified dispatcher with exact route-key matching.")
        appendLine(" *")
        appendLine(" * Local routes decode the payload, call the handler, and encode the response.")
        appendLine(" * Remote routes resolve the dependency alias, discover the target, and call raw.")
        appendLine(" */")
        appendLine("suspend fun actrDispatch(")
        appendLine("    ctx: ActrContext,")
        appendLine("    handler: ActrDispatchHandler,")
        appendLine("    routeKey: String,")
        appendLine("    payload: ByteArray,")
        appendLine("    timeoutMs: Long,")
        appendLine("): ByteArray = when (routeKey) {")
        append(cases)
        appendLine(elseCase)
        appendLine("}")
    }
}

/** Map the DispatchSpec wire value to the public Kotlin payload enum. */
private fun payloadTypeKotlin(value: String): String =
        when (value.lowercase()) {
            "rpcreliable" -> "PayloadType.RPC_RELIABLE"
            "rpcsignal" -> "PayloadType.RPC_SIGNAL"
            "streamreliable" -> "PayloadType.STREAM_RELIABLE"
            "streamlatencyfirst" -> "PayloadType.STREAM_LATENCY_FIRST"
            "mediartp" -> "PayloadType.MEDIA_RTP"
            else ->
                    throw IllegalArgumentException(
                            "Unsupported payload_type `$value` in DispatchSpec route")
        }


/** Build a Kotlin import path for a proto type owner. */
private fun buildImport(owner: TypeOwner): String {
    val packagePrefix = owner.javaPackage.ifEmpty { owner.packageName }
    if (owner.javaMultipleFiles) {
        return packagePrefix
    }
    val outer = owner.javaOuterClassName.ifEmpty {
        owner.protoFileName.substringAfterLast("/").removeSuffix(".proto").toPascalCase()
    }
    return if (packagePrefix.isEmpty()) outer else "$packagePrefix.$outer"
}

/** Resolve a fully-qualified proto type to its generated JVM type name. */
private fun resolveKotlinType(typeName: String, typeOwner: Map<String, TypeOwner>): String {
    val cleaned = typeName.trimStart('.')
    val owner = typeOwner[cleaned] ?: return cleaned
    val packagePrefix = owner.javaPackage.ifEmpty { owner.packageName }
    val messagePath =
            if (owner.messagePath.isEmpty()) listOf(cleaned.split(".").last())
            else owner.messagePath
    val classPath =
            if (owner.javaMultipleFiles) {
                messagePath.joinToString(".")
            } else {
                val outer = owner.javaOuterClassName.ifEmpty {
                    owner.protoFileName.substringAfterLast("/").removeSuffix(".proto").toPascalCase()
                }
                (listOf(outer) + messagePath).filter { it.isNotEmpty() }.joinToString(".")
            }
    return if (packagePrefix.isEmpty()) classPath else "$packagePrefix.$classPath"
}

/** Extract the service name from a fully-qualified proto_service like "echo.EchoService". */
private fun extractServiceName(protoService: String): String {
    return protoService.substringAfterLast(".")
}

/** Build the `actr-gen-meta.json` content with unified-dispatch fields. */
private fun buildMetadataJson(
        dispatchSpec: DispatchSpec,
        dispatchSpecHash: String,
        generatedFiles: List<String>,
        typeOwner: Map<String, TypeOwner>,
): String {
    val localRoutes = dispatchSpec.localRoutes()
    val remoteRoutes = dispatchSpec.remoteRoutes()
    val metadata =
            JSONObject()
                    .put("schema_version", "v1")
                    .put("plugin_version", "0.6.1")
                    .put("language", "kotlin")
                    .put("dispatch_spec_hash", dispatchSpecHash)
                    .put(
                            "generated_files",
                            jsonArray((generatedFiles + "actr-gen-meta.json").distinct().sorted()),
                    )
                    .put(
                            "generated_local_routes",
                            jsonArray(localRoutes.map { it.routeKey }.sorted()),
                    )
                    .put(
                            "generated_remote_routes",
                            jsonArray(remoteRoutes.map { it.routeKey }.sorted()),
                    )
                    .put("local_services", serviceMetadata(localRoutes, typeOwner, false))
                    .put("remote_services", serviceMetadata(remoteRoutes, typeOwner, true))
    return metadata.toString(2) + "\n"
}

private fun serviceMetadata(
        routes: List<DispatchRoute>,
        typeOwner: Map<String, TypeOwner>,
        remote: Boolean,
): JSONArray {
    val services = JSONArray()
    for ((protoService, serviceRoutes) in routes.groupBy { it.protoService }.toSortedMap()) {
        val separator = protoService.lastIndexOf('.')
        val packageName = if (separator < 0) "" else protoService.substring(0, separator)
        val serviceName = if (separator < 0) protoService else protoService.substring(separator + 1)
        val service =
                JSONObject()
                        .put("name", serviceName)
                        .put("package", packageName)
                        .put("proto_file", serviceRoutes.first().protoFile)
        if (remote) {
            service
                    .put("dependency_alias", serviceRoutes.first().dependencyAlias)
                    .put("client_type", "${serviceName}Client")
        } else {
            service.put("handler_interface", "${serviceName}Handler")
        }
        val methods = JSONArray()
        for (route in serviceRoutes.sortedBy { it.routeKey }) {
            methods.put(
                    JSONObject()
                            .put("name", route.method)
                            .put("snake_name", route.method.toSnakeCase())
                            .put("route_key", route.routeKey)
                            .put(
                                    "input_ref",
                                    metadataTypeRef(route.inputType, route.protoFile, typeOwner),
                            )
                            .put(
                                    "output_ref",
                                    metadataTypeRef(route.outputType, route.protoFile, typeOwner),
                            )
                            .put("reply_mode", route.replyMode)
            )
        }
        service.put("methods", methods)
        services.put(service)
    }
    return services
}

private fun metadataTypeRef(
        rawType: String,
        fallbackProtoFile: String,
        typeOwner: Map<String, TypeOwner>,
): JSONObject {
    val protoType = rawType.trimStart('.')
    val owner = typeOwner[protoType]
    val typeName = owner?.messagePath?.joinToString(".") ?: protoType.substringAfterLast('.')
    val packageName = owner?.packageName ?: protoType.substringBeforeLast('.', "")
    val protoFile = normalizeProtoPath(owner?.protoFileName ?: fallbackProtoFile)
    return JSONObject()
            .put("proto_type", protoType)
            .put("type_name", typeName)
            .put("proto_package", packageName)
            .put("proto_file", protoFile)
            .put("generated_type", resolveKotlinType(rawType, typeOwner))
}

private fun jsonArray(values: List<String>): JSONArray {
    val array = JSONArray()
    for (value in values) {
        array.put(value)
    }
    return array
}

private fun routeKey(packageName: String, serviceName: String, methodName: String): String {
    return if (packageName.isBlank()) "$serviceName.$methodName"
    else "$packageName.$serviceName.$methodName"
}

private fun normalizeProtoPath(value: String): String {
    var normalized = value.replace("\\", "/")
    while (normalized.startsWith("./")) {
        normalized = normalized.removePrefix("./")
    }
    return if (normalized.endsWith(".proto")) normalized else "$normalized.proto"
}

private fun kotlinStringLiteral(value: String): String {
    val result = StringBuilder("\"")
    for (char in value) {
        when (char) {
            '\b' -> result.append("\\b")
            '\t' -> result.append("\\t")
            '\n' -> result.append("\\n")
            '\u000C' -> result.append("\\f")
            '\r' -> result.append("\\r")
            '"' -> result.append("\\\"")
            '\\' -> result.append("\\\\")
            '$' -> result.append('\\').append('$')
            '\u2028', '\u2029' ->
                result.append("\\u").append(char.code.toString(16).padStart(4, '0'))
            else -> {
                if (char.code < 0x20) {
                    result.append("\\u").append(char.code.toString(16).padStart(4, '0'))
                } else {
                    result.append(char)
                }
            }
        }
    }
    return result.append('"').toString()
}

private fun collectTypeOwners(
        file: FileDescriptorProto,
        message: DescriptorProto,
        parentPath: List<String>,
        owners: MutableMap<String, TypeOwner>,
) {
    val messagePath = parentPath + message.name
    val protoName = (listOf(file.`package`).filter { it.isNotEmpty() } + messagePath).joinToString(".")
    owners[protoName] =
            TypeOwner(
                    packageName = file.`package`,
                    protoFileName = file.name,
                    javaPackage = file.options.javaPackage,
                    javaOuterClassName =
                            file.options.javaOuterClassname.ifEmpty { defaultJavaOuterClassName(file) },
                    javaMultipleFiles = file.options.javaMultipleFiles,
                    messagePath = messagePath,
            )
    for (nested in message.nestedTypeList) {
        collectTypeOwners(file, nested, messagePath, owners)
    }
}

private fun defaultJavaOuterClassName(file: FileDescriptorProto): String {
    val baseName = file.name.substringAfterLast("/").removeSuffix(".proto").toPascalCase()
    val topLevelNames =
            file.messageTypeList.map { it.name }.toSet() +
                    file.enumTypeList.map { it.name } +
                    file.serviceList.map { it.name }
    return if (baseName in topLevelNames) "${baseName}OuterClass" else baseName
}

private fun String.toPascalCase(): String {
    return this.split("_", "-").joinToString("") { word ->
        word.replaceFirstChar { it.uppercase() }
    }
}

private fun String.toSnakeCase(): String {
    return this.replace(Regex("(.)([A-Z][a-z]+)"), "$1_$2")
            .replace(Regex("([a-z0-9])([A-Z])"), "$1_$2")
            .lowercase()
}

/** Parse parameters from protoc --actrframework-kotlin_opt */
fun parseParameters(paramStr: String): Map<String, String> {
    return paramStr.split(",").filter { it.contains("=") }.associate {
        val (key, value) = it.split("=", limit = 2)
        key.trim() to value.trim()
    }
}
