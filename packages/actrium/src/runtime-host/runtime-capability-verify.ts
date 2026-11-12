import { contract } from './contract';
import {
  verifyCapabilityManifest,
  type CapabilityManifestV1,
  type CapabilityVerificationResult,
} from './capability-verify';
import { HOST_SUPPORTED_METHODS, HOST_SUPPORTED_MODULES } from './host-modules';

// Keep in sync with the default `requireWhitelist` in
// packages/keel/src/host/engine/engine.ts. Non-host imports outside this set
// are not provided by the Guest runtime and must fail before engine creation.
const GUEST_IMPORT_ALLOWLIST = new Set([
  'react',
  'react-native',
  'react/jsx-runtime',
  'react/jsx-dev-runtime',
  'keel/guest',
  'keel/reconciler',
]);

const contractKinds = Object.fromEntries(
  Object.entries(contract.hostModules).map(([moduleId, exports]) => [
    moduleId,
    Object.fromEntries(
      Object.entries(exports).map(([exportName, descriptor]) => [
        exportName,
        descriptor.kind,
      ])
    ),
  ])
);

/** Verify an unpacked manifest before the Guest Engine is created. */
export function verifyRuntimeCapabilityManifestJSON(
  manifestJSON: string,
  declaredCapabilities: readonly string[]
): CapabilityVerificationResult {
  try {
    const manifest = JSON.parse(manifestJSON) as CapabilityManifestV1;
    return verifyCapabilityManifest(manifest, {
      hostSupportedModules: HOST_SUPPORTED_MODULES,
      hostSupportedMethods: HOST_SUPPORTED_METHODS,
      allowedImports: GUEST_IMPORT_ALLOWLIST,
      contractKinds,
      declaredCapabilities,
    });
  } catch (cause) {
    return {
      ok: false,
      code: 'capability_mismatch',
      details: [
        `Invalid capability manifest: ${cause instanceof Error ? cause.message : String(cause)}`,
      ],
    };
  }
}
