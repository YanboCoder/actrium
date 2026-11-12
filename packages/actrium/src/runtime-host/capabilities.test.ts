import { describe, expect, test } from 'bun:test';
import {
  hasDeclaredCapability,
  resolveDeclaredCapability,
} from './capabilities';

describe('Front capability mapping', () => {
  test('resolves v1 capability names', () => {
    expect(resolveDeclaredCapability('service')).toBe('service');
    expect(resolveDeclaredCapability('host:clipboard')).toBe('host:clipboard');
    expect(resolveDeclaredCapability('host:net')).toBe('host:net');
    expect(resolveDeclaredCapability('host:navigate')).toBe('host:navigate');
  });

  test('maps the legacy actor.call alias to service', () => {
    expect(resolveDeclaredCapability('actor.call')).toBe('service');
  });

  test('does not treat ui.render as a Front capability', () => {
    expect(resolveDeclaredCapability('ui.render')).toBeUndefined();
  });

  test('detects declared capabilities through aliases', () => {
    expect(hasDeclaredCapability(['actor.call'], 'service')).toBe(true);
    expect(hasDeclaredCapability(['service'], 'service')).toBe(true);
    expect(hasDeclaredCapability(['ui.render'], 'service')).toBe(false);
    expect(hasDeclaredCapability([], 'service')).toBe(false);
  });
});
