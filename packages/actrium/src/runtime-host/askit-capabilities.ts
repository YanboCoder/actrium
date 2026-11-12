import type { FrontCapability } from './capabilities';

/**
 * Where askit's event-bridge modules belong in the capability model.
 *
 * `toast` and `haptic` are UI-only: they do not grant access to a Host module
 * or platform resource, so they must not appear in the Front capability list
 * (v1 06 §5.3). The askit runtime keeps its module-level permission names for
 * compatibility. This table is a classification aid: the askit event bridge is
 * not wired into the Actrium Host by design (Guests use the Keel contract), so
 * it has no production caller and is not the runtime source of truth.
 */
export type AskitModuleCapability = 'ui-only' | FrontCapability;

export const ASKIT_MODULE_CAPABILITIES: Record<string, AskitModuleCapability> = {
  toast: 'ui-only',
  haptic: 'ui-only',
};

export function resolveAskitModuleCapability(
  module: string
): AskitModuleCapability | undefined {
  return ASKIT_MODULE_CAPABILITIES[module];
}
