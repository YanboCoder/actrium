import type {
  ServiceCallInput,
  ServiceEvent,
  ServiceTellInput,
} from './contract';

export type ServiceCallRequest = ServiceCallInput;

export type ServiceCallResult = {
  success: boolean;
  payloadBase64?: string;
  message?: string;
};

export type ActorCall = (request: {
  requestId: string;
  route: string;
  payloadBase64: string;
  timeoutMs: number;
}) => Promise<ServiceCallResult>;

export type ActorTell = (request: {
  requestId: string;
  route: string;
  payloadBase64: string;
}) => Promise<ServiceCallResult>;

export type HostModules = {
  'host:service': {
    call: (input: ServiceCallRequest) => Promise<{ payloadBase64: string }>;
    tell: (input: ServiceTellInput) => Promise<void>;
    on: (
      handler: (event: ServiceEvent) => void
    ) => () => void;
  };
};

/** Methods with a real Host implementation, excluding throwing placeholders. */
export const HOST_SUPPORTED_METHODS: Readonly<Record<string, ReadonlySet<string>>> = {
  'host:service': new Set(['call', 'tell']),
};

/** Modules with a real Host implementation, not just a contract declaration. */
export const HOST_SUPPORTED_MODULES: ReadonlySet<string> = new Set(
  Object.keys(HOST_SUPPORTED_METHODS)
);

// `callActor` is expected to resolve with a failure result instead of
// rejecting after the race settles. HostHttpHandler catches transport errors,
// so a late rejection cannot surface as an unhandled rejection here.
function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`Service call timed out after ${timeoutMs} ms`)),
      timeoutMs
    );
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}

/**
 * Host-side implementations for the Guest contract. Keel validates this map
 * against the contract at Engine construction time, so a missing capability
 * fails closed before the Guest bundle runs.
 */
export function createHostModules(options: {
  hasServiceCapability: boolean;
  callActor: ActorCall;
  tellActor?: ActorTell;
}): HostModules {
  let requestSequence = 0;

  return {
    'host:service': {
      call: async (input: ServiceCallRequest) => {
        if (!options.hasServiceCapability) {
          throw new Error(
            'The application does not declare the service capability.'
          );
        }
        requestSequence += 1;
        const timeoutMs = input.timeoutMs ?? 30_000;
        const result = await withTimeout(
          options.callActor({
            requestId: `service-${Date.now()}-${requestSequence}`,
            route: input.route,
            payloadBase64: input.payloadBase64,
            timeoutMs,
          }),
          timeoutMs
        );
        if (!result.success || typeof result.payloadBase64 !== 'string') {
          throw new Error(result.message ?? 'Service call failed');
        }
        return { payloadBase64: result.payloadBase64 };
      },
      tell: async (input: ServiceTellInput) => {
        if (!options.hasServiceCapability) {
          throw new Error(
            'The application does not declare the service capability.'
          );
        }
        if (!options.tellActor) {
          throw new Error('host:service.tell is not available on this host.');
        }
        requestSequence += 1;
        const result = await options.tellActor({
          requestId: `service-tell-${Date.now()}-${requestSequence}`,
          route: input.route,
          payloadBase64: input.payloadBase64,
        });
        if (!result.success) {
          throw new Error(result.message ?? 'Service tell failed');
        }
      },
      on: () => {
        throw new Error('host:service.on is not implemented yet.');
      },
    },
  };
}
