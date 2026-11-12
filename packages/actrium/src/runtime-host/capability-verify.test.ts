import { describe, expect, test } from 'bun:test';
import {
  verifyCapabilityManifest,
  type CapabilityManifestV1,
} from './capability-verify';

const manifest: CapabilityManifestV1 = {
  schemaVersion: 1,
  keelApi: '1.0',
  guestEntry: 'ui/guest.bundle.js',
  imports: ['host:service'],
  hostModules: { service: { rpc: ['call'], events: [] } },
  components: ['View', 'Text'],
};

const options = {
  hostSupportedModules: new Set(['host:service']),
  contractKinds: { 'host:service': { call: 'rpc' as const } },
  declaredCapabilities: ['service'],
};

describe('verifyCapabilityManifest', () => {
  test('accepts a manifest that matches the host and package declaration', () => {
    expect(verifyCapabilityManifest(manifest, options)).toEqual({
      ok: true,
      details: [],
    });
  });

  test('accepts v1 business service routes with a service policy', () => {
    const result = verifyCapabilityManifest(
      {
        ...manifest,
        hostModules: {
          service: { rpc: ['echo.EchoService.Echo'], events: [] },
        },
      },
      {
        ...options,
        serviceCapabilities: {
          rpc: new Set(['echo.EchoService.Echo']),
          events: new Set(),
        },
      }
    );
    expect(result.ok).toBe(true);
  });

  test('rejects a service route outside the service policy', () => {
    const result = verifyCapabilityManifest(
      {
        ...manifest,
        hostModules: {
          service: { rpc: ['echo.EchoService.Echo'], events: [] },
        },
      },
      {
        ...options,
        serviceCapabilities: { rpc: new Set(['other.Route']), events: new Set() },
      }
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
  });

  test('enforces the service policy when contractKinds is omitted', () => {
    const result = verifyCapabilityManifest(
      {
        ...manifest,
        hostModules: {
          service: { rpc: ['admin.DeleteAll'], events: ['admin.secrets'] },
        },
      },
      {
        hostSupportedModules: new Set(['host:service']),
        declaredCapabilities: ['service'],
        serviceCapabilities: {
          rpc: new Set(['echo.EchoService.Echo']),
          events: new Set(),
        },
      }
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
    expect(result.details[0]).toContain('admin.DeleteAll');
  });

  test('enforces the per-app component allowlist', () => {
    expect(
      verifyCapabilityManifest(manifest, {
        ...options,
        appAllowComponents: ['View', 'Text'],
      }).ok
    ).toBe(true);

    const result = verifyCapabilityManifest(manifest, {
      ...options,
      appAllowComponents: ['View'],
    });
    expect(result.ok).toBe(false);
    expect(result.code).toBe('component_not_allowed');
  });

  test('an explicit empty app allowlist denies every component', () => {
    const result = verifyCapabilityManifest(manifest, {
      ...options,
      appAllowComponents: [],
    });
    expect(result.ok).toBe(false);
    expect(result.code).toBe('component_not_allowed');
  });

  test('tolerates duplicates in the app allowlist', () => {
    expect(
      verifyCapabilityManifest(manifest, {
        ...options,
        appAllowComponents: ['View', 'View', 'Text'],
      }).ok
    ).toBe(true);
  });

  test('rejects an app allowlist outside the host allowlist', () => {
    const result = verifyCapabilityManifest(manifest, {
      ...options,
      appAllowComponents: ['WebView'],
    });
    expect(result.ok).toBe(false);
    expect(result.code).toBe('component_not_allowed');
  });

  test('rejects a component outside the allowlist', () => {
    const result = verifyCapabilityManifest(
      { ...manifest, components: ['View', 'WebView'] },
      options
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('component_not_allowed');
  });

  test('rejects an unsupported host module import', () => {
    const result = verifyCapabilityManifest(
      { ...manifest, imports: ['host:service', 'host:unknown'] },
      options
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('unknown_capability');
  });

  test('rejects a host module used but not imported', () => {
    const result = verifyCapabilityManifest(
      {
        ...manifest,
        imports: [],
        hostModules: { service: { rpc: ['call'] } },
      },
      options
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
  });

  test('rejects an rpc name that the contract does not expose', () => {
    const result = verifyCapabilityManifest(
      {
        ...manifest,
        imports: ['host:service', 'host:clipboard'],
        hostModules: {
          ...manifest.hostModules,
          clipboard: { rpc: ['unknown'] },
        },
      },
      {
        ...options,
        hostSupportedModules: new Set(['host:service', 'host:clipboard']),
        contractKinds: {
          'host:service': { call: 'rpc' },
          'host:clipboard': { read: 'rpc' },
        },
        declaredCapabilities: ['service', 'host:clipboard'],
      }
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
  });

  test('rejects a host module the package did not declare', () => {
    const result = verifyCapabilityManifest(manifest, {
      ...options,
      declaredCapabilities: [],
    });
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
  });

  test('rejects an imported host module the package did not declare', () => {
    const result = verifyCapabilityManifest(
      {
        ...manifest,
        imports: ['host:service', 'host:clipboard'],
        hostModules: { service: { rpc: ['call'] } },
      },
      {
        ...options,
        declaredCapabilities: ['service'],
        hostSupportedModules: new Set(['host:service', 'host:clipboard']),
      }
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
  });

  test('rejects an unsupported manifest schema version', () => {
    const result = verifyCapabilityManifest(
      { ...manifest, schemaVersion: 2 },
      options
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('capability_mismatch');
  });

  test('rejects an import outside the allowed list', () => {
    const result = verifyCapabilityManifest(
      { ...manifest, imports: ['host:service', 'fs'] },
      { ...options, allowedImports: new Set(['keel/guest', 'react']) }
    );
    expect(result.ok).toBe(false);
    expect(result.code).toBe('unknown_capability');
  });

  test('accepts the legacy actor.call declaration as service', () => {
    expect(
      verifyCapabilityManifest(manifest, {
        ...options,
        declaredCapabilities: ['actor.call'],
      }).ok
    ).toBe(true);
  });
});
