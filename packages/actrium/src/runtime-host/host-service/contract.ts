/**
 * Keel contract for `host:service` (plan 15 IF-3 §2).
 *
 * The guest-facing surface is intentionally generic:
 *
 *   import { call, tell } from 'host:service';
 *   await call({ route, payloadBase64, timeoutMs });
 *
 * A generic envelope keeps the dynamic ACTR route inside one declared
 * capability. Per-route contract generation (one `rpc()` per service method)
 * is a later step; the transport underneath does not change.
 */

import { defineKeelContract, rpc } from 'keel/contract';

const DEFAULT_TIMEOUT_MS = 30_000;

function asRecord(value: unknown, field: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new TypeError(`${field} must be an object`);
  }
  return value as Record<string, unknown>;
}

function requiredString(
  record: Record<string, unknown>,
  field: string
): string {
  const value = record[field];
  if (typeof value !== 'string' || value.length === 0) {
    throw new TypeError(`${field} must be a non-empty string`);
  }
  return value;
}

function optionalTimeout(record: Record<string, unknown>): number {
  const value = record.timeoutMs;
  if (value === undefined) return DEFAULT_TIMEOUT_MS;
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
    throw new TypeError('timeoutMs must be a positive number');
  }
  return Math.floor(value);
}

export const actriumServiceContract = defineKeelContract({
  version: '1',
  hostModules: {
    'host:service': {
      // Reason: keel's `HostCapabilityDescriptor` widens to
      // `RpcDescriptor<unknown, unknown>`, and its `binary` metadata makes a
      // concrete descriptor invariant against that widening. The runtime schema
      // below still validates every boundary; concrete input/output types are
      // applied on the implementation side.
      call: rpc<unknown, unknown>({
        timeoutMs: DEFAULT_TIMEOUT_MS,
        schema: {
          parseInput: (value) => {
            const record = asRecord(value, 'service call input');
            return {
              route: requiredString(record, 'route'),
              payloadBase64: requiredString(record, 'payloadBase64'),
              timeoutMs: optionalTimeout(record),
            };
          },
          parseOutput: (value) => {
            const record = asRecord(value, 'service call output');
            return { payloadBase64: requiredString(record, 'payloadBase64') };
          },
        },
      }),
      tell: rpc<unknown, unknown>({
        timeoutMs: DEFAULT_TIMEOUT_MS,
        schema: {
          parseInput: (value) => {
            const record = asRecord(value, 'service tell input');
            return {
              route: requiredString(record, 'route'),
              payloadBase64: requiredString(record, 'payloadBase64'),
            };
          },
        },
      }),
    },
  },
  guestExports: {},
});
