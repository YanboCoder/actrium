import { describe, expect, test } from 'bun:test';

import { verifyRuntimeCapabilityManifestJSON } from './runtime-capability-verify';

const validManifest = {
  schemaVersion: 1,
  keelApi: '1.0',
  guestEntry: 'workload/guest.js',
  imports: ['host:service', 'keel/guest', 'react', 'react/jsx-runtime'],
  hostModules: { service: { rpc: ['counter.increment'], events: [] } },
  components: ['ScrollView', 'Text', 'TextInput', 'View'],
};

function manifestJSON(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({ ...validManifest, ...overrides });
}

describe('runtime capability manifest verification', () => {
  test('accepts business service routes that match declarations', () => {
    expect(verifyRuntimeCapabilityManifestJSON(manifestJSON(), ['service'])).toEqual({
      ok: true,
      details: [],
    });
  });

  test('accepts an empty hostModules manifest', () => {
    const result = verifyRuntimeCapabilityManifestJSON(
      manifestJSON({ hostModules: {} }),
      ['service']
    );
    expect(result).toEqual({ ok: true, details: [] });
  });

  test('rejects usage that is not declared by the package', () => {
    const result = verifyRuntimeCapabilityManifestJSON(manifestJSON(), []);
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
    expect(result.details.join('\n')).toContain('not declared');
  });

  test('rejects a module without a real Host implementation', () => {
    const result = verifyRuntimeCapabilityManifestJSON(
      manifestJSON({
        imports: ['host:clipboard'],
        hostModules: { 'host:clipboard': { rpc: ['writeText'] } },
      }),
      ['host:clipboard']
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('unknown_capability');
  });

  test('rejects an unsupported host module import', () => {
    const result = verifyRuntimeCapabilityManifestJSON(
      manifestJSON({ imports: ['host:unknown'], hostModules: {} }),
      ['host:unknown']
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('unknown_capability');
    expect(result.details.join('\n')).toContain('host:unknown');
  });

  test('rejects a non-host import the runtime cannot provide', () => {
    const result = verifyRuntimeCapabilityManifestJSON(
      manifestJSON({ imports: ['fs'], hostModules: {} }),
      ['service']
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('unknown_capability');
    expect(result.details.join('\n')).toContain('fs');
  });

  test('rejects a service event without an implemented subscription', () => {
    const result = verifyRuntimeCapabilityManifestJSON(
      manifestJSON({
        hostModules: {
          service: {
            rpc: ['counter.increment'],
            events: ['counter.changed'],
          },
        },
      }),
      ['service']
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('unknown_capability');
    expect(result.details.join('\n')).toContain('host:service.on');
  });

  test('rejects a component outside the host allowlist', () => {
    const result = verifyRuntimeCapabilityManifestJSON(
      manifestJSON({ components: ['CustomThing'] }),
      ['service']
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('component_not_allowed');
  });

  test('rejects an unsupported schema version', () => {
    const result = verifyRuntimeCapabilityManifestJSON(
      manifestJSON({ schemaVersion: 2 }),
      ['service']
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
    expect(result.details.join('\n')).toContain('schemaVersion');
  });

  test('rejects an empty manifest payload', () => {
    const result = verifyRuntimeCapabilityManifestJSON('', ['service']);
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
  });

  test('returns a validation failure for malformed JSON', () => {
    const result = verifyRuntimeCapabilityManifestJSON('{not json', ['service']);
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
    expect(result.details.join('\n')).toContain('Invalid capability manifest');
  });
});
