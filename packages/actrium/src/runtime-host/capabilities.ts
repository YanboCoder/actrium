/**
 * Front capability names and their Keel host-module mapping.
 *
 * Names follow the v1 runtime spec (06 §5.2/§5.3); the package manifest uses
 * the same strings in `[host].required-capabilities`, while the Keel contract
 * uses `host:<name>` module ids. `ui.render` is intentionally absent: it is a
 * component/rendering concern, not a callable Front capability.
 */

export const FRONT_CAPABILITIES = [
  'service',
  'host:assets',
  'host:clipboard',
  'host:notifications',
  'host:camera',
  'host:microphone',
  'host:location',
  'host:external-link',
  'host:navigate',
  'host:file-import',
  'host:file-export',
  'host:share',
  'host:net',
] as const;

export type FrontCapability = (typeof FRONT_CAPABILITIES)[number];

export const CAPABILITY_TO_HOST_MODULE: Record<FrontCapability, string> = {
  service: 'host:service',
  'host:assets': 'host:assets',
  'host:clipboard': 'host:clipboard',
  'host:notifications': 'host:notifications',
  'host:camera': 'host:camera',
  'host:microphone': 'host:microphone',
  'host:location': 'host:location',
  'host:external-link': 'host:external-link',
  'host:navigate': 'host:navigate',
  'host:file-import': 'host:file-import',
  'host:file-export': 'host:file-export',
  'host:share': 'host:share',
  'host:net': 'host:net',
};

/**
 * Legacy package names kept readable during the prototype migration.
 * `actor.call` was the pre-v1 name for the Service capability.
 */
export const LEGACY_CAPABILITY_ALIASES: Record<string, FrontCapability> = {
  'actor.call': 'service',
};

export function resolveDeclaredCapability(
  name: string
): FrontCapability | undefined {
  const alias = LEGACY_CAPABILITY_ALIASES[name];
  if (alias) return alias;
  return (FRONT_CAPABILITIES as readonly string[]).includes(name)
    ? (name as FrontCapability)
    : undefined;
}

export function hasDeclaredCapability(
  declared: readonly string[],
  capability: FrontCapability
): boolean {
  return declared.some(
    (name) => resolveDeclaredCapability(name) === capability
  );
}
