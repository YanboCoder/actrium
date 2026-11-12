import { describe, expect, test } from 'bun:test';
import { createHostModules } from './host-modules';

const input = { route: 'echo.EchoService.Echo', payloadBase64: 'AAEC' };

describe('host:service implementation', () => {
  test('rejects calls when the service capability is not declared', async () => {
    let called = false;
    const hostModules = createHostModules({
      hasServiceCapability: false,
      callActor: async () => {
        called = true;
        return { success: true, payloadBase64: 'AAEC' };
      },
    });

    await expect(hostModules['host:service'].call(input)).rejects.toThrow(
      'service capability'
    );
    expect(called).toBe(false);
  });

  test('returns the response payload and forwards the route', async () => {
    let captured: {
      route: string;
      payloadBase64: string;
      timeoutMs: number;
    } | null = null;
    const hostModules = createHostModules({
      hasServiceCapability: true,
      callActor: async (request) => {
        captured = request;
        return { success: true, payloadBase64: 'AAEC' };
      },
    });

    await expect(hostModules['host:service'].call(input)).resolves.toEqual({
      payloadBase64: 'AAEC',
    });
    expect(captured).toMatchObject({
      route: 'echo.EchoService.Echo',
      timeoutMs: 30_000,
    });
  });

  test('propagates gateway errors', async () => {
    const hostModules = createHostModules({
      hasServiceCapability: true,
      callActor: async () => ({ success: false, message: 'gateway down' }),
    });

    await expect(hostModules['host:service'].call(input)).rejects.toThrow(
      'gateway down'
    );
  });

  test('times out a call that never resolves', async () => {
    const hostModules = createHostModules({
      hasServiceCapability: true,
      callActor: () => new Promise(() => {}),
    });

    await expect(
      hostModules['host:service'].call({ ...input, timeoutMs: 10 })
    ).rejects.toThrow('timed out');
  });

  test('rejects a success response without a payload', async () => {
    const hostModules = createHostModules({
      hasServiceCapability: true,
      callActor: async () => ({ success: true }),
    });

    await expect(hostModules['host:service'].call(input)).rejects.toThrow(
      'Service call failed'
    );
  });

  test('tell forwards to the tab-scoped transport', async () => {
    const captured: { route?: string; payloadBase64?: string } = {};
    const hostModules = createHostModules({
      hasServiceCapability: true,
      callActor: async () => ({ success: true }),
      tellActor: async (request) => {
        captured.route = request.route;
        captured.payloadBase64 = request.payloadBase64;
        return { success: true };
      },
    });

    await expect(hostModules['host:service'].tell(input)).resolves.toBeUndefined();
    expect(captured).toEqual({
      route: 'echo.EchoService.Echo',
      payloadBase64: 'AAEC',
    });
  });

  test('fails closed when tell is not wired to a transport', async () => {
    const hostModules = createHostModules({
      hasServiceCapability: true,
      callActor: async () => ({ success: true }),
    });

    await expect(hostModules['host:service'].tell(input)).rejects.toThrow(
      'not available on this host'
    );
  });

  test('rejects tell when the service capability is not declared', async () => {
    let called = false;
    const hostModules = createHostModules({
      hasServiceCapability: false,
      callActor: async () => ({ success: true }),
      tellActor: async () => {
        called = true;
        return { success: true };
      },
    });

    await expect(hostModules['host:service'].tell(input)).rejects.toThrow(
      'service capability'
    );
    expect(called).toBe(false);
  });

  test('on fails closed until events are implemented', () => {
    const hostModules = createHostModules({
      hasServiceCapability: true,
      callActor: async () => ({ success: true }),
    });

    expect(() => hostModules['host:service'].on(() => {})).toThrow(
      'not implemented'
    );
  });
});
