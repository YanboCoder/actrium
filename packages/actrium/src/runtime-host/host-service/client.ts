/**
 * Transport for `host:service`.
 *
 * The Guest runs in the Renderer process, which cannot reach the Actr runtime
 * directly. Two interchangeable transports carry a call to the tab's Service
 * actor:
 *
 *  1. `NativeModules.ActrServiceModule` — the RN native module that forwards
 *     the call across the Renderer boundary to the per-tab loopback endpoint
 *     registered by the Host.
 *  2. The tab's `serviceCallUrl` over loopback HTTP — used on platforms (or
 *     test hosts) without the native module.
 *
 * There is deliberately no global gateway fallback: when neither transport is
 * available the call fails closed with `SERVICE_NOT_FOUND`, so a stale tab can
 * never silently talk to an unrelated actor.
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

type GatewayResponse = {
  success?: boolean;
  payloadBase64?: string;
  error?: string;
};

type NativeActrServiceModule = {
  call?: (
    tabId: string,
    route: string,
    payloadBase64: string,
    timeoutMs: number
  ) => Promise<unknown>;
  tell?: (
    tabId: string,
    route: string,
    payloadBase64: string
  ) => Promise<unknown>;
};

/** Identifies which tab's Service actor the call belongs to. */
export type ServiceCallContext = {
  tabId: string;
  serviceCallUrl?: string;
};

let requestSequence = 0;

function nextRequestId(): string {
  requestSequence += 1;
  return `service-call-${requestSequence}`;
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
  // An empty protobuf message encodes to "", which is a valid payload; only a
  // missing/non-string value is malformed.
  if (typeof value !== 'string') {
    throw new ActrServiceError(
      'INVALID_RESPONSE',
      'Actr service returned no payload.'
    );
  }
  return { payloadBase64: value };
}

function tellURL(serviceCallUrl: string): string {
  try {
    const url = new URL(serviceCallUrl);
    url.pathname = '/v1/tell';
    return url.toString();
  } catch {
    return serviceCallUrl;
  }
}

async function callViaNative(
  module: NativeActrServiceModule,
  context: ServiceCallContext,
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
      context.tabId,
      input.route,
      input.payloadBase64,
      input.timeoutMs ?? 30_000
    );
    return decodePayload(payload);
  } catch (cause) {
    throw asError(cause);
  }
}

async function tellViaNative(
  module: NativeActrServiceModule,
  context: ServiceCallContext,
  input: ServiceTellInput
): Promise<void> {
  if (!module.tell) {
    throw new ActrServiceError(
      'TRANSPORT_ERROR',
      'ActrServiceModule has no tell method.'
    );
  }
  try {
    await module.tell(context.tabId, input.route, input.payloadBase64);
  } catch (cause) {
    throw asError(cause);
  }
}

function gatewayErrorCode(message: string | undefined): ActrServiceErrorCode {
  return message?.toLowerCase().includes('timed out') ? 'TIMED_OUT' : 'TRANSPORT_ERROR';
}

async function callViaHTTP(
  serviceCallUrl: string,
  input: ServiceCallInput
): Promise<ServiceCallOutput> {
  const response = await HostHttpHandler.handleRequest<GatewayResponse>({
    requestId: nextRequestId(),
    url: serviceCallUrl,
    method: 'POST',
    body: {
      routeKey: input.route,
      payloadBase64: input.payloadBase64,
      timeoutMs: input.timeoutMs ?? 30_000,
    },
  });

  if (!response.success) {
    throw new ActrServiceError(
      gatewayErrorCode(response.message),
      response.message ?? 'Actr service request failed.'
    );
  }
  if (response.data?.success !== true) {
    throw new ActrServiceError(
      'INTERNAL',
      response.data?.error ?? 'Actr service returned a failed result.'
    );
  }

  return decodePayload(response.data?.payloadBase64);
}

async function tellViaHTTP(
  serviceCallUrl: string,
  input: ServiceTellInput
): Promise<void> {
  const response = await HostHttpHandler.handleRequest<GatewayResponse>({
    requestId: nextRequestId(),
    url: tellURL(serviceCallUrl),
    method: 'POST',
    body: {
      routeKey: input.route,
      payloadBase64: input.payloadBase64,
    },
  });

  if (!response.success) {
    throw new ActrServiceError(
      gatewayErrorCode(response.message),
      response.message ?? 'Actr service tell failed.'
    );
  }
  if (response.data?.success !== true) {
    throw new ActrServiceError(
      'INTERNAL',
      response.data?.error ?? 'Actr service returned a failed tell.'
    );
  }
}

function serviceNotFound(context: ServiceCallContext): ActrServiceError {
  return new ActrServiceError(
    'SERVICE_NOT_FOUND',
    `No Service actor endpoint is attached to tab ${context.tabId}.`
  );
}

export const actrServiceClient = {
  async call(
    input: ServiceCallInput,
    context: ServiceCallContext
  ): Promise<ServiceCallOutput> {
    const module = nativeModule();
    if (module?.call) return callViaNative(module, context, input);
    if (context.serviceCallUrl) {
      return callViaHTTP(context.serviceCallUrl, input);
    }
    throw serviceNotFound(context);
  },

  async tell(
    input: ServiceTellInput,
    context: ServiceCallContext
  ): Promise<void> {
    const module = nativeModule();
    if (module?.tell) return tellViaNative(module, context, input);
    if (context.serviceCallUrl) {
      return tellViaHTTP(context.serviceCallUrl, input);
    }
    throw serviceNotFound(context);
  },
};
