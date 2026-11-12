import { implementHostModules } from 'keel/contract';

import { actrServiceClient } from './client';
import { actriumServiceContract } from './contract';
import type { ServiceCallInput, ServiceTellInput } from './types';

/**
 * Host implementation paired with {@link actriumServiceContract}.
 *
 * The Engine wraps each of these with the contract's boundary schemas, so the
 * guest never reaches `actrServiceClient` with an unvalidated envelope.
 */
export const actriumHostModules = implementHostModules(
  actriumServiceContract,
  {
    'host:service': {
      // The dispatch wrapper runs the contract's parseInput before these, so
      // the concrete shape is guaranteed and the casts stay local.
      call: (input) => actrServiceClient.call(input as ServiceCallInput),
      tell: (input) => actrServiceClient.tell(input as ServiceTellInput),
    },
  }
);
