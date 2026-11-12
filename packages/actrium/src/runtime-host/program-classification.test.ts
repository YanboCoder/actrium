import { describe, expect, test } from 'bun:test';
import { classifyProgram, type ProgramClassificationInput } from './program-classification';

const systemBase: ProgramClassificationInput = {
  signature: 'first_party',
  policy: 'system',
};

describe('classifyProgram', () => {
  test('classifies first-party programs with an explicit system policy', () => {
    expect(classifyProgram(systemBase)).toBe('system');
  });

  test('rejects first-party programs without a system policy', () => {
    expect(classifyProgram({ ...systemBase, policy: 'user' })).toBe(
      'third_party'
    );
    expect(classifyProgram({ ...systemBase, policy: 'none' })).toBe(
      'third_party'
    );
  });

  test('rejects self-reported systemAppType', () => {
    expect(
      classifyProgram({
        signature: 'dev',
        policy: 'none',
        systemAppType: 'settings',
      })
    ).toBe('third_party');
  });

  test('rejects the publisher name alone', () => {
    expect(
      classifyProgram({
        signature: 'dev',
        policy: 'none',
        publisher: 'actrium',
      })
    ).toBe('third_party');
  });

  test('rejects the install directory alone', () => {
    expect(
      classifyProgram({
        signature: 'dev',
        policy: 'none',
        installDirectory: '/Applications/Actrium.app/Contents/Resources',
      })
    ).toBe('third_party');
  });

  test('rejects a development key even with a system policy claim', () => {
    expect(
      classifyProgram({ signature: 'dev', policy: 'system' })
    ).toBe('third_party');
  });

  test('rejects unknown signatures', () => {
    expect(
      classifyProgram({ signature: 'unknown', policy: 'system' })
    ).toBe('third_party');
  });
});
