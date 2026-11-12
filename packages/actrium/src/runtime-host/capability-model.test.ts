import { describe, expect, test } from 'bun:test';
import { FRONT_CAPABILITIES } from './capabilities';
import {
  computeFrontCapabilities,
  computeServiceCapabilities,
  FRONT_CAPABILITY_DEFINITIONS,
  PLATFORM_PERMISSION_MAP,
  type FrontCapabilityInput,
} from './capability-model';

const all = new Set<string>(FRONT_CAPABILITIES);

const base: FrontCapabilityInput = {
  declared: ['service', 'host:assets'],
  hostSupported: all,
  guestKind: 'js',
  grants: new Set<string>(),
  platform: all,
  session: all,
  audience: 'third_party',
};

describe('computeFrontCapabilities', () => {
  test('keeps declared capabilities with no grant requirement', () => {
    expect(computeFrontCapabilities(base)).toEqual(['service', 'host:assets']);
  });

  test('drops capabilities that require a grant until granted', () => {
    expect(
      computeFrontCapabilities({ ...base, declared: ['host:clipboard'] })
    ).toEqual([]);
    expect(
      computeFrontCapabilities({
        ...base,
        declared: ['host:clipboard'],
        grants: new Set(['host:clipboard']),
      })
    ).toEqual(['host:clipboard']);
  });

  test('drops capabilities the host does not support', () => {
    expect(
      computeFrontCapabilities({
        ...base,
        declared: ['service'],
        hostSupported: new Set(['host:clipboard']),
      })
    ).toEqual([]);
  });

  test('drops capabilities the platform or session does not allow', () => {
    expect(
      computeFrontCapabilities({
        ...base,
        declared: ['service'],
        platform: new Set(['host:assets']),
      })
    ).toEqual([]);
    expect(
      computeFrontCapabilities({
        ...base,
        declared: ['service'],
        session: new Set(['host:assets']),
      })
    ).toEqual([]);
  });

  test('maps the legacy actor.call alias to service', () => {
    expect(
      computeFrontCapabilities({ ...base, declared: ['actor.call'] })
    ).toEqual(['service']);
  });

  test('deduplicates a capability declared through aliases', () => {
    expect(
      computeFrontCapabilities({ ...base, declared: ['service', 'actor.call'] })
    ).toEqual(['service']);
  });

  test('accepts contract module ids in hostSupported', () => {
    expect(
      computeFrontCapabilities({
        ...base,
        declared: ['service'],
        hostSupported: new Set(['host:service']),
      })
    ).toEqual(['service']);
  });

  test('silently drops unknown capabilities', () => {
    expect(
      computeFrontCapabilities({ ...base, declared: ['unknown:thing'] })
    ).toEqual([]);
  });

  test('keeps third-party capabilities for the system audience', () => {
    expect(
      computeFrontCapabilities({
        ...base,
        declared: ['service'],
        audience: 'system',
      })
    ).toEqual(['service']);
  });
});

describe('capability model data', () => {
  test('every front capability has a definition', () => {
    for (const capability of FRONT_CAPABILITIES) {
      expect(FRONT_CAPABILITY_DEFINITIONS[capability]?.name).toBe(capability);
    }
  });

  test('host:net is default-off and broker-mediated', () => {
    expect(FRONT_CAPABILITY_DEFINITIONS['host:net'].defaultEnabled).toBe(false);
    expect(FRONT_CAPABILITY_DEFINITIONS['host:net'].requiresBroker).toBe(true);
  });

  test('clipboard has a platform permission mapping', () => {
    expect(PLATFORM_PERMISSION_MAP['host:clipboard']).toBe('os:clipboard');
  });

  test('host:navigate is available without a grant', () => {
    expect(FRONT_CAPABILITY_DEFINITIONS['host:navigate'].defaultEnabled).toBe(
      true
    );
    expect(
      computeFrontCapabilities({ ...base, declared: ['host:navigate'] })
    ).toEqual(['host:navigate']);
  });

  test('host:external-link requires a user grant', () => {
    expect(
      FRONT_CAPABILITY_DEFINITIONS['host:external-link'].requiresGrant
    ).toBe('user');
    expect(
      computeFrontCapabilities({ ...base, declared: ['host:external-link'] })
    ).toEqual([]);
    expect(
      computeFrontCapabilities({
        ...base,
        declared: ['host:external-link'],
        grants: new Set(['host:external-link']),
      })
    ).toEqual(['host:external-link']);
  });
});

describe('computeServiceCapabilities', () => {
  const declared = [
    'workspace:read-write',
    'actor:call:notes.example:index-worker:1',
    'shell:bookmarks',
  ];

  test('intersects declared, host-supported, policy and session', () => {
    expect(
      computeServiceCapabilities({
        declared,
        hostSupported: new Set(declared),
        policy: new Set(declared),
        session: new Set(declared),
      })
    ).toEqual(declared);
  });

  test('drops malformed service capabilities', () => {
    const malformed = [
      'workspace:write',
      'net:http://insecure.example',
      'actor:call:notes.example:index-worker',
      'shell:unknown',
    ];
    expect(
      computeServiceCapabilities({
        declared: malformed,
        hostSupported: new Set(malformed),
        policy: new Set(malformed),
        session: new Set(malformed),
      })
    ).toEqual([]);
  });

  test('drops capabilities missing from policy or session', () => {
    expect(
      computeServiceCapabilities({
        declared,
        hostSupported: new Set(declared),
        policy: new Set(['workspace:read-write']),
        session: new Set(declared),
      })
    ).toEqual(['workspace:read-write']);
  });
});
