import { defineKeelContract, rpc, subscription } from 'keel/contract';

/**
 * Guest-side service capability contract (v1 runtime spec, section 5.2).
 *
 * The host fixes the target to the current Service ActorId; the Guest never
 * supplies an ActorId. Business routes stay opaque strings here and are
 * checked against the package capability manifest once route gating lands.
 */

export type ServiceCallInput = {
  route: string;
  payloadBase64: string;
  timeoutMs?: number;
};

export type ServiceCallOutput = {
  payloadBase64: string;
};

export type ServiceTellInput = {
  route: string;
  payloadBase64: string;
};

export type ServiceEvent = {
  event: string;
  payloadBase64: string;
};

function asObject(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== 'object') {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function requireString(value: unknown, label: string, allowEmpty = false): string {
  if (typeof value !== 'string' || (!allowEmpty && value.length === 0)) {
    throw new Error(
      `${label} must be ${allowEmpty ? 'a string' : 'a non-empty string'}`
    );
  }
  return value;
}

const BASE64_PATTERN =
  /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

/** Upper bound so a Guest cannot overflow the host setTimeout clamp. */
const MAX_SERVICE_TIMEOUT_MS = 300_000;

function requireBase64(value: unknown, label: string): string {
  const text = requireString(value, label, true);
  if (!BASE64_PATTERN.test(text)) {
    throw new Error(
      `${label} must be standard base64 (RFC 4648, padded)`
    );
  }
  return text;
}

export const contract = defineKeelContract({
  version: '1.0.0',
  hostModules: {
    'host:service': {
      call: rpc<ServiceCallInput, ServiceCallOutput>({
        schema: {
          parseInput: (value) => {
            const input = asObject(value, 'service.call input');
            const route = requireString(input.route, 'service.call input.route');
            const payloadBase64 = requireBase64(
              input.payloadBase64,
              'service.call input.payloadBase64'
            );
            const { timeoutMs } = input;
            if (
              timeoutMs !== undefined &&
              (typeof timeoutMs !== 'number' ||
                !Number.isFinite(timeoutMs) ||
                timeoutMs <= 0 ||
                timeoutMs > MAX_SERVICE_TIMEOUT_MS)
            ) {
              throw new Error(
                `service.call input.timeoutMs must be a positive number no greater than ${MAX_SERVICE_TIMEOUT_MS} when present`
              );
            }
            return { route, payloadBase64, timeoutMs };
          },
          parseOutput: (value) => {
            const output = asObject(value, 'service.call output');
            return {
              payloadBase64: requireBase64(
                output.payloadBase64,
                'service.call output.payloadBase64'
              ),
            };
          },
        },
      }),
      tell: rpc<ServiceTellInput, void>({
        schema: {
          parseInput: (value) => {
            const input = asObject(value, 'service.tell input');
            return {
              route: requireString(input.route, 'service.tell input.route'),
              payloadBase64: requireBase64(
                input.payloadBase64,
                'service.tell input.payloadBase64'
              ),
            };
          },
        },
      }),
      on: subscription<ServiceEvent>({
        schema: {
          parseEvent: (value) => {
            const event = asObject(value, 'service.on event');
            return {
              event: requireString(event.event, 'service.on event.event'),
              payloadBase64: requireBase64(
                event.payloadBase64,
                'service.on event.payloadBase64'
              ),
            };
          },
        },
      }),
    },
  },
  guestExports: {},
});
