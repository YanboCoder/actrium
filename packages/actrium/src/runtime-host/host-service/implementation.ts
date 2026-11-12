import { implementHostModules } from 'keel/contract';

import { actrServiceClient, type ServiceCallContext } from './client';
import { actriumServiceContract } from './contract';
import type { ServiceCallInput, ServiceTellInput } from './types';

/**
 * Host implementation paired with {@link actriumServiceContract}.
 *
 * The Engine wraps each of these with the contract's boundary schemas, so the
 * guest never reaches `actrServiceClient` with an unvalidated envelope. The
 * module is bound to one tab: `host:service` routes to that tab's Service
 * actor only, never to a global endpoint.
 */
export function createActriumHostModules(context: ServiceCallContext) {
  return implementHostModules(actriumServiceContract, {
    'host:service': {
      // The dispatch wrapper runs the contract's parseInput before these, so
      // the concrete shape is guaranteed and the casts stay local.
      call: (input) =>
        actrServiceClient.call(input as ServiceCallInput, context),
      tell: (input) =>
        actrServiceClient.tell(input as ServiceTellInput, context),
    },
  });
}
