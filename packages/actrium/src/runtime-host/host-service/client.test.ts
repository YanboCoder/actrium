import { beforeEach, describe, expect, mock, test } from 'bun:test';

type HttpRequest = {
  requestId: string;
  url: string;
  method: string;
  body: Record<string, unknown>;
};

type HttpResponse = {
  success: boolean;
  message?: string;
  data?: {
    success?: boolean;
    payloadBase64?: string;
    error?: string;
  };
};

// The transport selects between the React Native native module and loopback
// HTTP. Both boundaries are replaced here so the selection order and the
// fail-closed behaviour are exercised without a running host.
const nativeModules: Record<string, unknown> = {};
const httpRequests: HttpRequest[] = [];
let httpResponse: HttpResponse = {
  success: true,
  data: { success: true, payloadBase64: 'AAEC' },
};

mock.module('react-native', () => ({ NativeModules: nativeModules }));
mock.module('askit/host-events', () => ({
  HostHttpHandler: {
    handleRequest: async (request: HttpRequest) => {
      httpRequests.push(request);
      return httpResponse;
    },
  },
}));

const { actrServiceClient } = await import('./client');

const input = { route: 'echo.EchoService.Echo', payloadBase64: 'AAEC' };
const context = {
  tabId: 'tab-1',
  serviceCallUrl: 'http://127.0.0.1:5000/v1/call',
};

describe('host:service client', () => {
  beforeEach(() => {
    for (const key of Object.keys(nativeModules)) delete nativeModules[key];
    httpRequests.length = 0;
    httpResponse = {
      success: true,
      data: { success: true, payloadBase64: 'AAEC' },
    };
  });

  test('prefers the native module and passes the tab id', async () => {
    let captured: unknown[] = [];
    nativeModules.ActrServiceModule = {
      call: async (...args: unknown[]) => {
        captured = args;
        return 'AAEC';
      },
    };

    await expect(actrServiceClient.call(input, context)).resolves.toEqual({
      payloadBase64: 'AAEC',
    });
    expect(captured).toEqual([
      'tab-1',
      'echo.EchoService.Echo',
      'AAEC',
      30_000,
    ]);
    expect(httpRequests).toHaveLength(0);
  });

  test('falls back to the tab serviceCallUrl without a native module', async () => {
    await expect(actrServiceClient.call(input, context)).resolves.toEqual({
      payloadBase64: 'AAEC',
    });

    expect(httpRequests).toHaveLength(1);
    expect(httpRequests[0].url).toBe('http://127.0.0.1:5000/v1/call');
    expect(httpRequests[0].body).toEqual({
      routeKey: 'echo.EchoService.Echo',
      payloadBase64: 'AAEC',
      timeoutMs: 30_000,
    });
  });

  test('fails closed with SERVICE_NOT_FOUND when no endpoint is attached', async () => {
    await expect(
      actrServiceClient.call(input, { tabId: 'tab-1' })
    ).rejects.toMatchObject({
      name: 'ActrError',
      code: 'SERVICE_NOT_FOUND',
    });
    expect(httpRequests).toHaveLength(0);
  });

  test('propagates the native stable error code', async () => {
    nativeModules.ActrServiceModule = {
      call: async () => {
        throw Object.assign(new Error('timed out'), { code: 'TIMED_OUT' });
      },
    };

    await expect(actrServiceClient.call(input, context)).rejects.toMatchObject({
      code: 'TIMED_OUT',
    });
  });

  test('maps tell to the tab serviceCallUrl /v1/tell endpoint', async () => {
    await expect(
      actrServiceClient.tell({ route: input.route, payloadBase64: 'AAEC' }, context)
    ).resolves.toBeUndefined();

    expect(httpRequests).toHaveLength(1);
    expect(httpRequests[0].url).toBe('http://127.0.0.1:5000/v1/tell');
  });

  test('maps a loopback timeout to TIMED_OUT', async () => {
    httpResponse = {
      success: false,
      message: 'Request timed out after 30000 ms',
    };

    await expect(actrServiceClient.call(input, context)).rejects.toMatchObject({
      code: 'TIMED_OUT',
    });
  });
});
