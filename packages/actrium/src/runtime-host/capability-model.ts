import {
  CAPABILITY_TO_HOST_MODULE,
  FRONT_CAPABILITIES,
  resolveDeclaredCapability,
  type FrontCapability,
} from './capabilities';
import { isValidServiceCapability } from './service-capabilities';

/**
 * Capability layering and effective-set computation.
 *
 * The four layers follow v1 05 §7 / 06 §5.3:
 *   1. Front capability     — callable Host modules (`service`, `host:*`)
 *   2. Component whitelist  — Keel components a Guest may render
 *   3. Platform permission  — OS-level grants (camera/mic/location/notifications)
 *   4. Service capability   — workspace/network/actor/shell capabilities
 *
 * The attribute set follows `capability-tiers.zh.md` §4 (audience, grant,
 * defaults, broker, sandbox). Capability data is intentionally data-only so
 * the system-app audience can reuse the same structures later.
 */

export type CapabilityAudience = 'third_party' | 'system';
export type CapabilityGrant = 'none' | 'user' | 'admin' | 'elevated';

export type CapabilityDefinition = {
  name: string;
  audience: CapabilityAudience;
  requiresGrant: CapabilityGrant;
  defaultEnabled: boolean;
  /** Consumed at call time by the broker; not part of the effective set. */
  requiresBroker: boolean;
  /** Consumed by the sandbox at call time; not part of the effective set. */
  requiresSandbox: boolean;
};

export const FRONT_CAPABILITY_DEFINITIONS: Record<
  FrontCapability,
  CapabilityDefinition
> = {
  service: {
    name: 'service',
    audience: 'third_party',
    requiresGrant: 'none',
    defaultEnabled: true,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:assets': {
    name: 'host:assets',
    audience: 'third_party',
    requiresGrant: 'none',
    defaultEnabled: true,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:clipboard': {
    name: 'host:clipboard',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:notifications': {
    name: 'host:notifications',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:camera': {
    name: 'host:camera',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:microphone': {
    name: 'host:microphone',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:location': {
    name: 'host:location',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:external-link': {
    name: 'host:external-link',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:navigate': {
    name: 'host:navigate',
    audience: 'third_party',
    requiresGrant: 'none',
    defaultEnabled: true,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:file-import': {
    name: 'host:file-import',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:file-export': {
    name: 'host:file-export',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:share': {
    name: 'host:share',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: false,
    requiresSandbox: true,
  },
  'host:net': {
    name: 'host:net',
    audience: 'third_party',
    requiresGrant: 'user',
    defaultEnabled: false,
    requiresBroker: true,
    requiresSandbox: true,
  },
};

export const COMPONENT_WHITELIST = [
  'View',
  'Text',
  'ScrollView',
  'TextInput',
  'Button',
  'FlatList',
  'ImageAsset',
  'Switch',
] as const;

export type AllowedComponent = (typeof COMPONENT_WHITELIST)[number];

/** Platform permissions are a separate layer from front capabilities. */
export const PLATFORM_PERMISSION_MAP: Record<string, string> = {
  'host:clipboard': 'os:clipboard',
  'host:camera': 'os:camera',
  'host:microphone': 'os:microphone',
  'host:location': 'os:location',
  'host:notifications': 'os:notifications',
};

function normalizeCapabilityName(name: string): FrontCapability | undefined {
  const resolved = resolveDeclaredCapability(name);
  if (resolved) return resolved;
  for (const [capability, moduleId] of Object.entries(
    CAPABILITY_TO_HOST_MODULE
  )) {
    if (moduleId === name) return capability as FrontCapability;
  }
  return undefined;
}

export type FrontCapabilityInput = {
  declared: readonly string[];
  /**
   * Explicit capability names (`service`, `host:clipboard`, ...). Contract
   * module ids such as `host:service` are accepted too and normalized.
   */
  hostSupported: ReadonlySet<string>;
  /** Reserved for native Hosts; v1 currently opens JS Guests only. */
  guestKind: 'js' | 'native';
  grants: ReadonlySet<string>;
  platform: ReadonlySet<string>;
  session: ReadonlySet<string>;
  audience: CapabilityAudience;
};

/**
 * effective = declared ∩ host-supported ∩ audience ∩ grant ∩ platform ∩ session.
 * Parsing route names stays opaque; the result is the callable set.
 */
export function computeFrontCapabilities(
  input: FrontCapabilityInput
): FrontCapability[] {
  const supported = new Set<FrontCapability>();
  for (const name of input.hostSupported) {
    const capability = normalizeCapabilityName(name);
    if (capability) supported.add(capability);
  }

  const result: FrontCapability[] = [];
  const seen = new Set<FrontCapability>();

  for (const name of input.declared) {
    const capability = normalizeCapabilityName(name);
    if (!capability || seen.has(capability)) continue;
    const definition = FRONT_CAPABILITY_DEFINITIONS[capability];
    if (!definition) continue;
    if (!supported.has(capability)) continue;
    if (definition.audience === 'system' && input.audience !== 'system') continue;
    // `grants` holds capabilities whose required grant level was satisfied.
    // A default-off capability additionally needs an explicit grant.
    const needsGrant =
      definition.requiresGrant !== 'none' || !definition.defaultEnabled;
    if (needsGrant && !input.grants.has(capability)) continue;
    if (!input.platform.has(capability)) continue;
    if (!input.session.has(capability)) continue;
    seen.add(capability);
    result.push(capability);
  }

  return result;
}

export type ServiceCapabilityInput = {
  declared: readonly string[];
  hostSupported: ReadonlySet<string>;
  policy: ReadonlySet<string>;
  session: ReadonlySet<string>;
};

/**
 * Service capabilities are per-instance. v1 currently opens JS Guests only, so
 * the result is the third-party set; system instances will add their own
 * audience rule when that path exists.
 */
export function computeServiceCapabilities(
  input: ServiceCapabilityInput
): string[] {
  return input.declared.filter(
    (capability) =>
      isValidServiceCapability(capability) &&
      input.hostSupported.has(capability) &&
      input.policy.has(capability) &&
      input.session.has(capability)
  );
}
