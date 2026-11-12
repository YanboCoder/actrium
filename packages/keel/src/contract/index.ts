/**
 * keel/contract
 *
 * Single source of truth for Host capabilities and Guest exports.
 */

export type HostModuleId = `host:${string}`;

export interface BoundarySchema<Input, Output> {
  // Reason: Boundary schemas parse untrusted cross-runtime input.
  parseInput?: (value: unknown) => Input;
  // Reason: Boundary schemas parse untrusted cross-runtime output.
  parseOutput?: (value: unknown) => Output;
}

/**
 * Names of the top-level keys of `T` whose value is (or includes) a `Uint8Array`
 * byte stream. Used to constrain {@link BinaryFields} so a declared binary field
 * name is checked against the actual Input/Output type where feasible: a field
 * typed `string`/`number`/etc. is rejected, an unknown field name is rejected.
 * Falls back to `never` when `T` is not an object (e.g. `void`/`unknown`).
 *
 * A field that can be ABSENT (optional, or typed `| undefined` / `| null`) must
 * be declared with a trailing `?` marker (`'body?'`): the runtime backstop then
 * tolerates absence but still requires a `Uint8Array` when the field is
 * present. A required field is declared without the marker, so its absence
 * stays a boundary violation.
 */
export type BinaryFieldNames<T> = T extends object
  ? {
      [K in keyof T]-?: K extends string
        ? Uint8Array extends NonNullable<T[K]>
          ? Extract<T[K], undefined | null> extends never
            ? K
            : `${K}?`
          : never
        : never;
    }[keyof T]
  : never;

/**
 * ADDITIVE, OPTIONAL metadata: declares which top-level request/response fields
 * carry a raw byte stream (a `Uint8Array`) rather than JSON scalar data.
 *
 * It is metadata only — it does NOT drive the wire codec (the KBS1 envelope is
 * self-describing via `{"$b":N}` sentinels, see `contracts/sandbox/store-net-bytes.json`).
 * It is consumed by: (a) type generation (a listed field maps to `Uint8Array` in
 * the emitted `declare module 'host:*'`), (b) the capabilities manifest
 * (`binaryCapabilities`), and (c) the dispatch boundary (a declared binary field
 * must be an actual `Uint8Array`, never a number-array — the fail-closed backstop
 * for the self-describing wire).
 */
export interface BinaryFields<Input = unknown, Output = unknown> {
  /** Top-level request field names whose value is a byte stream (`Uint8Array`). */
  input?: readonly BinaryFieldNames<Input>[];
  /** Top-level response field names whose value is a byte stream (`Uint8Array`). */
  output?: readonly BinaryFieldNames<Output>[];
}

/**
 * The subscription-side sibling of {@link BinaryFields}: a subscription has no
 * request/response pair, only a host->guest EVENT, so it declares one direction.
 * Same three consumers (type generation, `binaryCapabilities`, the dispatch
 * boundary backstop) and the same self-describing KBS1 wire.
 */
export interface BinaryEventFields<Event = unknown> {
  /** Top-level event field names whose value is a byte stream (`Uint8Array`). */
  event?: readonly BinaryFieldNames<Event>[];
}

/**
 * Which guest runtime a host capability can actually be reached from.
 *
 * - `js` — the interpreted guest (QuickJS / Hermes), which holds real function
 *   references and so can pass a handler across the boundary.
 * - `native` — the wasm guest, whose only route into a host module is
 *   `keel_host_call` with a byte payload (JSON or a KBS1 envelope).
 *
 * This is a STRUCTURAL fact of each mechanism, not a policy the contract author
 * picks — see {@link guestKindsFor}.
 */
export type GuestKind = 'js' | 'native';

export interface RpcOptions<Input, Output> {
  timeoutMs?: number;
  schema?: BoundarySchema<Input, Output>;
  binary?: BinaryFields<Input, Output>;
}

export interface RpcDescriptor<Input = void, Output = void> {
  readonly kind: 'rpc';
  readonly timeoutMs?: number;
  readonly schema?: BoundarySchema<Input, Output>;
  readonly binary?: BinaryFields<Input, Output>;
  readonly __input?: Input;
  readonly __output?: Output;
}

export interface SubscriptionOptions<Event> {
  schema?: {
    // Reason: Subscription schemas parse untrusted cross-runtime events.
    parseEvent?: (value: unknown) => Event;
  };
  binary?: BinaryEventFields<Event>;
}

export interface SubscriptionDescriptor<Event = unknown> {
  readonly kind: 'subscription';
  readonly schema?: {
    // Reason: Subscription schemas parse untrusted cross-runtime events.
    parseEvent?: (value: unknown) => Event;
  };
  readonly binary?: BinaryEventFields<Event>;
  readonly __event?: Event;
}

// The concrete Input/Output/Event types are carried by each contract's own
// descriptors and inferred by `HostModuleImplementation`. This alias only
// constrains the runtime shape, so the phantom `__input`/`__output`/`__event`
// fields must not force every concrete descriptor through `unknown`.
// biome-ignore lint/suspicious/noExplicitAny: concrete descriptor types are inferred at the implementation boundary
export type HostCapabilityDescriptor = RpcDescriptor<any, any> | SubscriptionDescriptor<any>;

export type HostModuleSpec = Record<string, HostCapabilityDescriptor>;

export interface KeelContractShape {
  version: string;
  hostModules: Record<HostModuleId, HostModuleSpec>;
  guestExports: Record<string, RpcDescriptor<unknown, unknown>>;
}

export type RpcInput<T> = T extends RpcDescriptor<infer Input, unknown> ? Input : never;

export type RpcOutput<T> = T extends RpcDescriptor<unknown, infer Output> ? Output : never;

export type SubscriptionEvent<T> = T extends SubscriptionDescriptor<infer Event> ? Event : never;

// biome-ignore lint/suspicious/noConfusingVoidType: rpc<void, T> is the public no-input contract spelling.
type RpcArgs<Input> = [Input] extends [void] ? [] : [input: Input];

export type RpcImplementation<Input, Output> = (
  ...args: RpcArgs<Input>
) => Output | Promise<Output>;

export type SubscriptionImplementation<Event> = (
  handler: (event: Event) => void
) => undefined | (() => void);

export type HostModuleImplementation<TModule extends HostModuleSpec> = {
  [Key in keyof TModule]: TModule[Key] extends RpcDescriptor<infer Input, infer Output>
    ? RpcImplementation<Input, Output>
    : TModule[Key] extends SubscriptionDescriptor<infer Event>
      ? SubscriptionImplementation<Event>
      : never;
};

export type HostModulesImplementation<TContract extends KeelContractShape> = {
  [ModuleId in keyof TContract['hostModules']]: TContract['hostModules'][ModuleId] extends HostModuleSpec
    ? HostModuleImplementation<TContract['hostModules'][ModuleId]>
    : never;
};

/**
 * Structural (non-generic) shape of a host module implementation map.
 *
 * Used by the runtime backend (Engine) where the concrete contract type is not
 * statically available. `(...args: never[]) => unknown` is the top function type
 * under contravariance, so any concrete `HostModulesImplementation<TContract>` is
 * assignable to it.
 */
export type HostModuleImplementationMap = Record<
  string,
  Record<string, (...args: never[]) => unknown>
>;

export type GuestExportsClient<TContract extends KeelContractShape> = {
  [ExportName in keyof TContract['guestExports']]: TContract['guestExports'][ExportName] extends RpcDescriptor<
    infer Input,
    infer Output
  >
    ? (...args: RpcArgs<Input>) => Promise<Output>
    : never;
};

export interface KeelCapabilitiesManifest {
  contractVersion: string;
  hostCapabilities: string[];
  guestExports: string[];
  /**
   * Declared host capabilities whose untrusted guest->host input is NOT validated:
   * rpc capabilities missing `parseInput`, or subscription capabilities missing
   * `parseEvent`. (`parseOutput` is host->guest and not required.) A sealed-tier
   * publish gate should reject a non-empty list.
   */
  unschemed: string[];
  /**
   * Host capabilities (`moduleId.export`) that declare at least one binary
   * (byte-stream) input or output field via `rpc({ binary })`. ADDITIVE and
   * OPTIONAL: present only when the contract has a binary surface, so a manifest
   * of a purely-JSON contract is byte-for-byte identical to before. Lets a
   * publish gate see which capabilities exchange raw bytes. Existing readers
   * ignore it.
   */
  binaryCapabilities?: string[];
  /**
   * Host capabilities (`moduleId.export`) that only an interpreted (JS) guest
   * can reach — a native (wasm) guest structurally cannot, see
   * {@link guestKindsFor}. ADDITIVE and OPTIONAL: present only when the contract
   * declares such a capability, so an rpc-only contract's manifest is unchanged.
   * A host shipping a native guest should treat a non-empty list as "these are
   * unavailable to it", and reach that guest through the named event channel
   * instead. Existing readers ignore it.
   */
  jsOnlyCapabilities?: string[];
}

/**
 * A single dispatch-wrapped host capability, ready to be exposed to the Guest.
 *
 * Wrapping enforces the contract's boundary schemas: rpc capabilities run
 * `parseInput` before the implementation and `parseOutput` on its result;
 * subscription capabilities run `parseEvent` on every event before it reaches
 * the Guest handler. Validation failures throw (fail-closed).
 */
// Reason: dispatch handlers cross the runtime boundary with arbitrary argument shapes.
export type HostModuleDispatchHandler = (...args: unknown[]) => unknown;

/** Dispatch-wrapped capabilities of a single host module, keyed by export name. */
export type HostModuleDispatchModule = Record<string, HostModuleDispatchHandler>;

/** Dispatch-wrapped host modules, keyed by host module id (e.g. `host:analytics`). */
export type HostModuleDispatchTable = Record<string, HostModuleDispatchModule>;

/** Boundary phase at which a dispatch wrapper rejected a value. */
export type HostModuleBoundaryPhase = 'input' | 'output' | 'event';

export interface HostModuleDispatchContext {
  moduleId: string;
  exportName: string;
  phase: HostModuleBoundaryPhase;
}

export interface HostModuleDispatchOptions {
  /**
   * Invoked when a boundary schema rejects a value. The dispatch wrapper still
   * throws after this hook runs; the hook is for host-side diagnostics/logging.
   */
  onError?: (error: Error, context: HostModuleDispatchContext) => void;
}

const HOST_MODULE_ID_PATTERN = /^host:[a-z0-9_-]+(?:\/[a-z0-9_-]+)*$/;
const GUEST_EXPORT_NAME_PATTERN = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

export function rpc<Input = void, Output = void>(
  options: RpcOptions<Input, Output> = {}
): RpcDescriptor<Input, Output> {
  return freezeDescriptor({
    kind: 'rpc',
    timeoutMs: options.timeoutMs,
    schema: options.schema,
    binary: options.binary,
  });
}

export function subscription<Event = unknown>(
  options: SubscriptionOptions<Event> = {}
): SubscriptionDescriptor<Event> {
  return freezeDescriptor({
    kind: 'subscription',
    schema: options.schema,
    binary: options.binary,
  });
}

/**
 * The guest kinds a capability can actually be reached from — DERIVED from the
 * mechanism, never declared, so it cannot drift from what the runtime does:
 *
 * - `rpc` works from both guest kinds: its argument is data, and data crosses
 *   either boundary (JSON, or a KBS1 envelope when it carries bytes).
 * - `subscription` is `js`-ONLY. Subscribing means handing the host a HANDLER
 *   FUNCTION, and a native guest's only route into a host module is
 *   `keel_host_call`, which carries bytes — a decoded byte payload is never a
 *   function. So the call cannot be expressed, let alone dispatched. The
 *   host->guest push channel for a native guest is the named event channel
 *   (`NativeGuestView.sendEvent` -> the guest's `keel_on_event`), which since the
 *   KBS1 event fork carries JSON *and* raw bytes.
 *
 * Consumed by publish gates and type generators (via the manifest's
 * `jsOnlyCapabilities`) so the limit is visible BEFORE a native guest hits it.
 */
export function guestKindsFor(descriptor: HostCapabilityDescriptor): readonly GuestKind[] {
  return descriptor.kind === 'subscription' ? JS_ONLY_GUEST_KINDS : ALL_GUEST_KINDS;
}

const ALL_GUEST_KINDS: readonly GuestKind[] = Object.freeze(['js', 'native'] as const);
const JS_ONLY_GUEST_KINDS: readonly GuestKind[] = Object.freeze(['js'] as const);

export function isHostModuleId(value: string): value is HostModuleId {
  return HOST_MODULE_ID_PATTERN.test(value);
}

export function assertHostModuleId(value: string): asserts value is HostModuleId {
  if (!isHostModuleId(value)) {
    throw new Error(
      `[keel/contract] Invalid host module id "${value}". Expected format: host:<domain>[/subdomain].`
    );
  }
}

export function defineKeelContract<const TContract extends KeelContractShape>(
  contract: TContract
): Readonly<TContract> {
  validateContract(contract);
  return deepFreeze(contract);
}

export function implementHostModules<const TContract extends KeelContractShape>(
  contract: TContract,
  implementation: HostModulesImplementation<TContract>
): HostModulesImplementation<TContract> {
  validateHostModulesImplementation(contract, implementation);
  return implementation;
}

/**
 * Build the dispatch table that the runtime backend exposes to the Guest.
 *
 * This is the runtime counterpart of {@link implementHostModules}: it pairs each
 * contract descriptor with its implementation and returns boundary-enforcing
 * wrappers. The Engine injects the result into the sandbox as
 * `globalThis.__keel.hostModules`, where the Guest's rewritten `host:*` imports
 * resolve it.
 *
 * Enforcement (fail-closed):
 * - rpc: `schema.parseInput(args)` runs before the implementation; `schema.parseOutput(result)`
 *   runs on the resolved value (awaited for async implementations). Either failing throws.
 * - subscription: `schema.parseEvent(event)` runs on every event before it reaches the Guest
 *   handler; a rejected event throws and never reaches the Guest.
 * - The implementation's own errors propagate unchanged.
 *
 * The implementation must exactly match the contract (validated here), so a guest
 * capability that is declared but not implemented is impossible; an undeclared
 * `host:*` import is rejected at build time and, defensively, by the runtime resolver.
 */
export function createHostModuleDispatch(
  contract: KeelContractShape,
  implementation: HostModuleImplementationMap,
  options: HostModuleDispatchOptions = {}
): HostModuleDispatchTable {
  assertImplementationMatchesContract(contract, implementation);

  const implementationRecord = implementation as Record<
    string,
    Record<string, HostModuleDispatchHandler>
  >;
  const table: HostModuleDispatchTable = {};

  for (const [moduleId, moduleSpec] of Object.entries(contract.hostModules)) {
    const moduleImpl = implementationRecord[moduleId];
    if (!moduleImpl) continue;

    const moduleDispatch: HostModuleDispatchModule = {};

    for (const [exportName, descriptor] of Object.entries(moduleSpec)) {
      const impl = moduleImpl[exportName];
      if (typeof impl !== 'function') continue;

      moduleDispatch[exportName] =
        descriptor.kind === 'subscription'
          ? wrapSubscriptionDispatch(moduleId, exportName, descriptor, impl, options)
          : wrapRpcDispatch(moduleId, exportName, descriptor, impl, options);
    }

    table[moduleId] = moduleDispatch;
  }

  return table;
}

export function createCapabilitiesManifest(contract: KeelContractShape): KeelCapabilitiesManifest {
  validateContract(contract);

  const hostCapabilities = Object.entries(contract.hostModules)
    .flatMap(([moduleId, moduleSpec]) =>
      Object.keys(moduleSpec).map((exportName) => `${moduleId}.${exportName}`)
    )
    .sort();

  const guestExports = Object.keys(contract.guestExports).sort();

  // A capability's untrusted input is validated when rpc has parseInput or
  // subscription has parseEvent. (parseOutput is host->guest and not required.)
  const unschemed = Object.entries(contract.hostModules)
    .flatMap(([moduleId, moduleSpec]) =>
      Object.entries(moduleSpec)
        .filter(([, descriptor]) =>
          descriptor.kind === 'subscription'
            ? !descriptor.schema?.parseEvent
            : !descriptor.schema?.parseInput
        )
        .map(([exportName]) => `${moduleId}.${exportName}`)
    )
    .sort();

  // Host capabilities declaring any byte-stream field. Additive: only emitted
  // when non-empty so a purely-JSON contract's manifest is unchanged.
  const binaryCapabilities = Object.entries(contract.hostModules)
    .flatMap(([moduleId, moduleSpec]) =>
      Object.entries(moduleSpec)
        .filter(([, descriptor]) => descriptorHasBinary(descriptor))
        .map(([exportName]) => `${moduleId}.${exportName}`)
    )
    .sort();

  // Host capabilities a NATIVE (wasm) guest structurally cannot reach — today
  // exactly the subscriptions (see guestKindsFor). Additive: only emitted when
  // non-empty, so an rpc-only contract's manifest is unchanged.
  const jsOnlyCapabilities = Object.entries(contract.hostModules)
    .flatMap(([moduleId, moduleSpec]) =>
      Object.entries(moduleSpec)
        .filter(([, descriptor]) => !guestKindsFor(descriptor).includes('native'))
        .map(([exportName]) => `${moduleId}.${exportName}`)
    )
    .sort();

  const manifest: KeelCapabilitiesManifest = {
    contractVersion: contract.version,
    hostCapabilities,
    guestExports,
    unschemed,
  };

  if (binaryCapabilities.length > 0) {
    manifest.binaryCapabilities = binaryCapabilities;
  }

  if (jsOnlyCapabilities.length > 0) {
    manifest.jsOnlyCapabilities = jsOnlyCapabilities;
  }

  return manifest;
}

/** True when a descriptor declares at least one binary field, in any direction. */
function descriptorHasBinary(descriptor: HostCapabilityDescriptor): boolean {
  if (descriptor.kind === 'subscription') {
    return (descriptor.binary?.event?.length ?? 0) > 0;
  }
  const binary = descriptor.binary;
  return Boolean(binary && ((binary.input?.length ?? 0) > 0 || (binary.output?.length ?? 0) > 0));
}

export function validateContract(contract: KeelContractShape): void {
  if (!contract || typeof contract !== 'object') {
    throw new Error('[keel/contract] Contract must be an object.');
  }

  if (!contract.version || typeof contract.version !== 'string') {
    throw new Error('[keel/contract] Contract version must be a non-empty string.');
  }

  validateHostModules(contract.hostModules);
  validateGuestExports(contract.guestExports);
}

function validateHostModules(hostModules: KeelContractShape['hostModules']): void {
  if (!hostModules || typeof hostModules !== 'object') {
    throw new Error('[keel/contract] hostModules must be an object.');
  }

  for (const [moduleId, moduleSpec] of Object.entries(hostModules)) {
    assertHostModuleId(moduleId);

    if (!moduleSpec || typeof moduleSpec !== 'object') {
      throw new Error(`[keel/contract] Host module "${moduleId}" must be an object.`);
    }

    for (const [exportName, descriptor] of Object.entries(moduleSpec)) {
      validateExportName(exportName, `host module "${moduleId}"`);
      validateDescriptor(descriptor, `${moduleId}.${exportName}`);
    }
  }
}

function validateGuestExports(guestExports: KeelContractShape['guestExports']): void {
  if (!guestExports || typeof guestExports !== 'object') {
    throw new Error('[keel/contract] guestExports must be an object.');
  }

  for (const [exportName, descriptor] of Object.entries(guestExports)) {
    validateExportName(exportName, 'guest exports');

    if (descriptor.kind !== 'rpc') {
      throw new Error(`[keel/contract] Guest export "${exportName}" must be rpc().`);
    }

    validateDescriptor(descriptor, `guest.${exportName}`);
  }
}

function validateHostModulesImplementation<const TContract extends KeelContractShape>(
  contract: TContract,
  implementation: HostModulesImplementation<TContract>
): void {
  assertImplementationMatchesContract(contract, implementation as HostModuleImplementationMap);
}

/**
 * Assert that an implementation map exactly covers a contract's host modules:
 * every declared module/export has a function implementation, and no extra
 * (undeclared) modules or exports are present.
 */
function assertImplementationMatchesContract(
  contract: KeelContractShape,
  implementation: HostModuleImplementationMap
): void {
  validateContract(contract);

  const implementationRecord = implementation as Record<string, Record<string, unknown>>;

  for (const moduleId of Object.keys(implementationRecord)) {
    if (!(moduleId in contract.hostModules)) {
      throw new Error(`[keel/contract] Host module implementation "${moduleId}" is not declared.`);
    }
  }

  for (const [moduleId, moduleSpec] of Object.entries(contract.hostModules)) {
    const moduleImpl = implementationRecord[moduleId];

    if (!moduleImpl || typeof moduleImpl !== 'object') {
      throw new Error(`[keel/contract] Missing implementation for host module "${moduleId}".`);
    }

    for (const exportName of Object.keys(moduleImpl)) {
      if (!(exportName in moduleSpec)) {
        throw new Error(
          `[keel/contract] Host module implementation "${moduleId}.${exportName}" is not declared.`
        );
      }
    }

    for (const exportName of Object.keys(moduleSpec)) {
      const impl = moduleImpl[exportName];

      if (typeof impl !== 'function') {
        throw new Error(
          `[keel/contract] Missing function implementation for "${moduleId}.${exportName}".`
        );
      }
    }
  }
}

function validateDescriptor(
  // Reason: Descriptor validation accepts arbitrary runtime values before narrowing.
  value: unknown,
  label: string
): asserts value is HostCapabilityDescriptor {
  if (!value || typeof value !== 'object') {
    throw new Error(
      `[keel/contract] Descriptor "${label}" must be created with rpc() or subscription().`
    );
  }

  // Reason: Descriptor kind is read from an arbitrary runtime value after object narrowing.
  const kind = (value as { kind?: unknown }).kind;

  if (kind !== 'rpc' && kind !== 'subscription') {
    throw new Error(`[keel/contract] Descriptor "${label}" must be rpc() or subscription().`);
  }

  // Reason: `binary` is optional metadata on either descriptor kind; when set it
  // must be a well-formed field-name map whose directions match that kind.
  validateBinaryFields((value as { binary?: unknown }).binary, label, kind);
}

/**
 * Validate an optional `binary` descriptor field: when present it must be an
 * object whose direction lists (each optional) are arrays of non-empty
 * field-name strings. Purely additive — an absent `binary` is always valid.
 *
 * The directions are kind-specific: an rpc has `input`/`output`, a subscription
 * has only `event`. Declaring a direction the kind does not have is rejected —
 * it would be silently ignored otherwise, and a binary field that no backstop
 * checks is exactly the fail-open case the backstop exists to prevent.
 */
// Reason: validates a descriptor's optional `binary` field, whose shape is unverified until checked here.
function validateBinaryFields(value: unknown, label: string, kind: 'rpc' | 'subscription'): void {
  if (value === undefined) {
    return;
  }

  if (!value || typeof value !== 'object') {
    throw new Error(`[keel/contract] Descriptor "${label}" binary must be an object.`);
  }

  // Reason: narrowing the validated object; each direction is re-checked below.
  const binary = value as { input?: unknown; output?: unknown; event?: unknown };
  const allowed = kind === 'subscription' ? (['event'] as const) : (['input', 'output'] as const);

  for (const direction of ['input', 'output', 'event'] as const) {
    if (binary[direction] !== undefined && !(allowed as readonly string[]).includes(direction)) {
      throw new Error(
        `[keel/contract] Descriptor "${label}" binary.${direction} is not valid on a ${kind} descriptor (expected ${allowed.join('/')}).`
      );
    }
  }

  for (const direction of allowed) {
    const list = binary[direction];
    if (list === undefined) {
      continue;
    }

    if (!Array.isArray(list)) {
      throw new Error(
        `[keel/contract] Descriptor "${label}" binary.${direction} must be an array of field names.`
      );
    }

    for (const name of list) {
      // A trailing '?' is the optional-field marker; the name before it must
      // still be non-empty (a bare '?' names no field).
      const fieldName = typeof name === 'string' && name.endsWith('?') ? name.slice(0, -1) : name;
      if (typeof fieldName !== 'string' || fieldName.length === 0) {
        throw new Error(
          `[keel/contract] Descriptor "${label}" binary.${direction} must contain non-empty field-name strings.`
        );
      }
    }
  }
}

function validateExportName(value: string, owner: string): void {
  if (!GUEST_EXPORT_NAME_PATTERN.test(value)) {
    throw new Error(`[keel/contract] Invalid export name "${value}" in ${owner}.`);
  }
}

function wrapRpcDispatch(
  moduleId: string,
  exportName: string,
  descriptor: RpcDescriptor<unknown, unknown>,
  impl: HostModuleDispatchHandler,
  options: HostModuleDispatchOptions
): HostModuleDispatchHandler {
  const schema = descriptor.schema;
  // Reason: the erased descriptor exposes binary field names as `readonly string[]`.
  const binaryInput = descriptor.binary?.input as readonly string[] | undefined;
  const binaryOutput = descriptor.binary?.output as readonly string[] | undefined;
  const hasBinaryInput = Boolean(binaryInput && binaryInput.length > 0);
  const hasBinaryOutput = Boolean(binaryOutput && binaryOutput.length > 0);

  return (...args: unknown[]): unknown => {
    // Boundary: parse the untrusted input the Guest sent before it reaches the host impl.
    let callArgs = args;
    if (schema?.parseInput) {
      const parsed = runBoundary(
        schema.parseInput,
        args[0],
        moduleId,
        exportName,
        'input',
        options
      );
      // Preserve runtime-owned invocation context after the guest-controlled
      // first argument. Native hosts use the second argument for an AbortSignal;
      // dropping it here made cancellation disappear precisely on capabilities
      // that had the strongest boundary schemas. Only args[0] is guest input and
      // therefore only args[0] is parsed.
      callArgs = [parsed, ...args.slice(1)];
    }

    // Boundary backstop: a declared-binary input field must be a real Uint8Array
    // (the self-describing wire reconstructs it before dispatch); a number-array
    // or wrong type is rejected fail-closed before the impl runs.
    if (hasBinaryInput) {
      runBoundary(
        (value) => assertBinaryFields(value, binaryInput!, 'input'),
        callArgs[0],
        moduleId,
        exportName,
        'input',
        options
      );
    }

    const result = impl(...(callArgs as never[]));

    const parseOutput = schema?.parseOutput;
    if (!parseOutput && !hasBinaryOutput) {
      // Unchanged path: no output boundary work — byte-for-byte identical to before.
      return result;
    }

    // Boundary: parse (and/or binary-check) the host impl's output before it crosses
    // back to the Guest. parseOutput narrows first, then the binary backstop runs.
    // Reason: the host impl's output crosses the boundary untyped until parseOutput/binary checks run.
    const finalize = (value: unknown): unknown => {
      const parsed = parseOutput
        ? runBoundary(parseOutput, value, moduleId, exportName, 'output', options)
        : value;
      if (hasBinaryOutput) {
        runBoundary(
          (candidate) => assertBinaryFields(candidate, binaryOutput!, 'output'),
          parsed,
          moduleId,
          exportName,
          'output',
          options
        );
      }
      return parsed;
    };

    if (isThenable(result)) {
      return Promise.resolve(result).then(finalize);
    }

    return finalize(result);
  };
}

/**
 * Fail-closed backstop for declared-binary fields: assert every named field of
 * `value` is an actual `Uint8Array`. Returns the value unchanged on success so it
 * can be run through {@link runBoundary}. Throws a short message the boundary
 * wrapper decorates with the capability + phase context.
 *
 * A `null`/absent whole result is passed through unchecked: it carries no fields
 * to validate, so a nullable output that declares `binary` (e.g. a store's
 * `getBytes(key) -> { value: Uint8Array; ... } | null` returning `null` for a
 * missing key) is a legitimate result, not a boundary violation.
 */
// Reason: a fail-closed backstop over an untyped boundary value; returns it unchanged for runBoundary.
function assertBinaryFields(
  value: unknown,
  fields: readonly string[],
  phase: HostModuleBoundaryPhase
  // Reason: returns the value unchanged so runBoundary can thread it through the boundary.
): unknown {
  // No whole result -> no binary fields to assert. Without this, the field walk
  // below would reject a nullable output's legitimate `null` (or an absent input).
  if (value == null) {
    return value;
  }
  for (const field of fields) {
    // Trailing '?' marks an OPTIONAL binary field (e.g. an optional request
    // body): absence is legitimate, but a present value must still be a real
    // Uint8Array. Unmarked fields keep failing closed on absence.
    const optional = field.endsWith('?');
    const name = optional ? field.slice(0, -1) : field;
    const fieldValue = (value as Record<string, unknown> | null | undefined)?.[name];
    if (optional && fieldValue == null) {
      continue;
    }
    if (!(fieldValue instanceof Uint8Array)) {
      throw new Error(
        `binary ${phase} field "${name}" must be a Uint8Array${optional ? ' when present' : ''}`
      );
    }
  }
  return value;
}

function wrapSubscriptionDispatch(
  moduleId: string,
  exportName: string,
  descriptor: SubscriptionDescriptor,
  impl: HostModuleDispatchHandler,
  options: HostModuleDispatchOptions
): HostModuleDispatchHandler {
  const schema = descriptor.schema;
  // Reason: the erased descriptor exposes binary field names as `readonly string[]`.
  const binaryEvent = descriptor.binary?.event as readonly string[] | undefined;
  const hasBinaryEvent = Boolean(binaryEvent && binaryEvent.length > 0);

  return (...args: unknown[]): unknown => {
    const handler = args[0];
    if (typeof handler !== 'function') {
      // The only caller that can get here with a non-function is a NATIVE (wasm)
      // guest: its route into a host module is keel_host_call, which carries
      // bytes, and a decoded byte payload is never a function. Say that, rather
      // than blaming the argument — the guest kind is the reason, and the fix is
      // a different channel, not a different argument. See guestKindsFor.
      throw new Error(
        `[keel/contract] Subscription "${moduleId}.${exportName}" requires a handler function, so it is reachable only from a JS guest. A native (wasm) guest cannot subscribe (its calls carry bytes, not function references) — push to it over the named event channel instead.`
      );
    }

    // Reason: the Guest handler and its events cross the runtime boundary as untrusted values.
    const guestHandler = handler as (event: unknown) => unknown;
    const parseEvent = schema?.parseEvent;
    // Reason: an event from the host implementation is untrusted until parseEvent / the binary backstop runs.
    const wrappedHandler =
      parseEvent || hasBinaryEvent
        ? (event: unknown): unknown => {
            // Boundary: parse every event before it reaches the Guest handler,
            // then run the declared-binary backstop on what parsing produced
            // (same order as rpc's output path: narrow first, assert after).
            let checked = parseEvent
              ? runBoundary(parseEvent, event, moduleId, exportName, 'event', options)
              : event;
            if (hasBinaryEvent) {
              checked = runBoundary(
                (candidate) => assertBinaryFields(candidate, binaryEvent!, 'event'),
                checked,
                moduleId,
                exportName,
                'event',
                options
              );
            }
            // Preserve the handler's completion value. Most subscriptions are
            // synchronous and still return undefined, but transports that own
            // bounded receive credit need to await an isolated guest actually
            // processing the event before they admit more bytes.
            return guestHandler(checked);
          }
        : guestHandler;

    return impl(wrappedHandler as never);
  };
}

// Reason: boundary parsers receive arbitrary cross-runtime input and return a narrowed value.
function runBoundary(
  parse: (value: unknown) => unknown,
  value: unknown,
  moduleId: string,
  exportName: string,
  phase: HostModuleBoundaryPhase,
  // Reason: returns the parsed (narrowed) value, or throws when the boundary rejects it.
  options: HostModuleDispatchOptions
): unknown {
  try {
    return parse(value);
  } catch (cause) {
    const reason = cause instanceof Error ? cause.message : String(cause);
    const error = new Error(
      `[keel/contract] Boundary ${phase} validation failed for "${moduleId}.${exportName}": ${reason}`
    );
    // Reason: preserve the original validation error for host-side diagnostics.
    (error as { cause?: unknown }).cause = cause;
    options.onError?.(error, { moduleId, exportName, phase });
    throw error;
  }
}

// Reason: type guard narrows an arbitrary runtime value to a thenable.
function isThenable(value: unknown): value is PromiseLike<unknown> {
  return (
    value != null &&
    (typeof value === 'object' || typeof value === 'function') &&
    // Reason: probe the value's then property without assuming its shape.
    typeof (value as { then?: unknown }).then === 'function'
  );
}

function freezeDescriptor<T extends object>(descriptor: T): Readonly<T> {
  return Object.freeze(descriptor);
}

function deepFreeze<T>(value: T): Readonly<T> {
  if (!value || typeof value !== 'object' || Object.isFrozen(value)) {
    return value as Readonly<T>;
  }

  for (const key of Object.keys(value as Record<string, unknown>)) {
    const child = (value as Record<string, unknown>)[key];
    if (child && typeof child === 'object') {
      deepFreeze(child);
    }
  }

  return Object.freeze(value);
}
