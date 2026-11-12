/**
 * Transport for `host:service`.
 *
 * Phase 0 keeps two interchangeable backends:
 *
 *  1. `NativeModules.ActrServiceModule` — the RN native module that will carry
 *     service calls across the renderer/main boundary once the ActrNode host is
 *     linked in the main process.
 *  2. The local Actr HTTP gateway — the same endpoint the existing ACTR_CALL
 *     event handler uses. It keeps the bridge usable before the native module
 *     (and the Actr runtime) are available.
 *
 * The guest-facing call sites do not change when backend (2) is replaced by an
 * IPC-backed native module.
 */

import { HostHttpHandler } from 'askit/host-events';
import { NativeModules } from 'react-native';

import {
  ActrServiceError,
  type ActrServiceErrorCode,
  type ServiceCallInput,
  type ServiceCallOutput,
  type ServiceTellInput,
} from './types';

const DEFAULT_GATEWAY_URL = 'http://127.0.0.1:8391/v1/call';

type GatewayResponse = {
  success?: boolean;
  payloadBase64?: string;
  error?: string;
};

type NativeActrServiceModule = {
  call?: (
    route: string,
    payloadBase64: string,
    timeoutMs: number
  ) => Promise<unknown>;
  tell?: (route: string, payloadBase64: string) => Promise<unknown>;
};

let requestSequence = 0;

function nextRequestId(): string {
  requestSequence += 1;
  return `service-call-${requestSequence}`;
}

function gatewayURL(): string {
  return globalThis.__ACTRIUM_ACTR_GATEWAY_URL__ ?? DEFAULT_GATEWAY_URL;
}

function nativeModule(): NativeActrServiceModule | undefined {
  const candidate = (NativeModules as Record<string, unknown>)
    .ActrServiceModule;
  if (typeof candidate !== 'object' || candidate === null) return undefined;
  return candidate as NativeActrServiceModule;
}

function errorCode(value: unknown): ActrServiceErrorCode {
  switch (value) {
    case 'TRANSPORT_ERROR':
    case 'INVALID_RESPONSE':
    case 'SERVICE_NOT_FOUND':
    case 'TIMED_OUT':
    case 'INTERNAL':
      return value;
    default:
      return 'INTERNAL';
  }
}

function asError(cause: unknown): ActrServiceError {
  if (cause instanceof ActrServiceError) return cause;
  const message = cause instanceof Error ? cause.message : String(cause);
  const code = errorCode(
    typeof cause === 'object' && cause !== null
      ? (cause as { code?: unknown }).code
      : undefined
  );
  return new ActrServiceError(code, message);
}

function decodePayload(value: unknown): ServiceCallOutput {
  if (typeof value !== 'string' || value.length === 0) {
    throw new ActrServiceError(
      'INVALID_RESPONSE',
      'Actr service returned an empty payload.'
    );
  }
  return { payloadBase64: value };
}

async function callViaNative(
  module: NativeActrServiceModule,
  input: ServiceCallInput
): Promise<ServiceCallOutput> {
  if (!module.call) {
    throw new ActrServiceError(
      'TRANSPORT_ERROR',
      'ActrServiceModule has no call method.'
    );
  }
  try {
    const payload = await module.call(
      input.route,
      input.payloadBase64,
      input.timeoutMs ?? 30_000
    );
    return decodePayload(payload);
  } catch (cause) {
    throw asError(cause);
  }
}

async function callViaGateway(
  input: ServiceCallInput
): Promise<ServiceCallOutput> {
  const response = await HostHttpHandler.handleRequest<GatewayResponse>({
    requestId: nextRequestId(),
    url: gatewayURL(),
    method: 'POST',
    body: {
      routeKey: input.route,
      payloadBase64: input.payloadBase64,
      timeoutMs: input.timeoutMs ?? 30_000,
    },
  });

  if (!response.success) {
    throw new ActrServiceError(
      'TRANSPORT_ERROR',
      response.message ?? 'Actr gateway request failed.'
    );
  }
  if (response.data?.success !== true) {
    throw new ActrServiceError(
      'INTERNAL',
      response.data?.error ?? 'Actr gateway returned a failed result.'
    );
  }

  return decodePayload(response.data?.payloadBase64);
}

export const actrServiceClient = {
  async call(input: ServiceCallInput): Promise<ServiceCallOutput> {
    const module = nativeModule();
    if (module) return callViaNative(module, input);
    return callViaGateway(input);
  },

  async tell(input: ServiceTellInput): Promise<void> {
    const module = nativeModule();
    if (module?.tell) {
      try {
        await module.tell(input.route, input.payloadBase64);
        return;
      } catch (cause) {
        throw asError(cause);
      }
    }

    // The gateway currently exposes request/response only; a tell is a call
    // whose reply we intentionally discard.
    await callViaGateway({
      route: input.route,
      payloadBase64: input.payloadBase64,
    });
  },
};
