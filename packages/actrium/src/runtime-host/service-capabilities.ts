/**
 * Service capability grammar (v1 12 §4 / 06 §5.2).
 *
 * Service capabilities are per-instance strings, not module ids:
 *   workspace:read-only | workspace:read-write   (v1 08 access values)
 *   net:https://api.notes.example                (no userinfo; ports allowed)
 *   actor:call:notes.example:index-worker:1      (v1 12 §4 notation)
 *   shell:bookmarks | shell:history | shell:settings   (prototype extension)
 *
 * v1 09 §6.4 uses a dot notation (`actor.call:...@1`) while 12 §4 uses the
 * colon form above; this module follows 12 §4.
 */

export type ServiceCapabilityKind = 'workspace' | 'net' | 'actor' | 'shell';

export type ServiceCapabilityRule = {
  kind: ServiceCapabilityKind;
  pattern: RegExp;
};

export const SERVICE_CAPABILITY_RULES: readonly ServiceCapabilityRule[] = [
  { kind: 'workspace', pattern: /^workspace:(read-only|read-write)$/ },
  // Domain labels, optional in-range port and path are allowed; userinfo,
  // whitespace and fragments are not.
  {
    kind: 'net',
    pattern:
      /^net:https:\/\/[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*(?::(?:[1-9]\d{0,3}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5]))?(?:\/[^\s@#]*)?$/,
  },
  {
    kind: 'actor',
    pattern: /^actor:call:[A-Za-z0-9._-]+:[A-Za-z0-9._-]+:\d+$/,
  },
  { kind: 'shell', pattern: /^shell:(bookmarks|history|settings)$/ },
];

export function classifyServiceCapability(
  name: string
): ServiceCapabilityKind | undefined {
  return SERVICE_CAPABILITY_RULES.find((rule) => rule.pattern.test(name))
    ?.kind;
}

export function isValidServiceCapability(name: string): boolean {
  return classifyServiceCapability(name) !== undefined;
}
