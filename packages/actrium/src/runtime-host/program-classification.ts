export type ProgramAudience = 'third_party' | 'system';
export type ProgramSignature = 'first_party' | 'dev' | 'unknown';
export type HostPolicy = 'system' | 'user' | 'none';

export type ProgramClassificationInput = {
  signature: ProgramSignature;
  policy: HostPolicy;
  /** Self-reported metadata: never sufficient on its own (v1 02 §8). */
  systemAppType?: string;
  publisher?: string;
  installDirectory?: string;
};

/**
 * A program is system only when it has a first-party signature and an explicit
 * system Host policy. `systemAppType`, `publisher = "actrium"`, the install
 * directory and a development key are self-reported or weak signals and must
 * never classify a program as system by themselves.
 *
 * There is no production caller yet; the native loader starts consuming this
 * once package provenance verification lands.
 */
export function classifyProgram(
  input: ProgramClassificationInput
): ProgramAudience {
  return input.signature === 'first_party' && input.policy === 'system'
    ? 'system'
    : 'third_party';
}
