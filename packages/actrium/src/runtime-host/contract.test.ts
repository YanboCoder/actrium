import { describe, expect, test } from 'bun:test';
import { contract } from './contract';

type InputSchema = {
  schema?: {
    parseInput?: (value: unknown) => unknown;
    parseOutput?: (value: unknown) => unknown;
  };
};

type EventSchema = {
  schema?: {
    parseEvent?: (value: unknown) => unknown;
  };
};

const service = contract.hostModules['host:service'];
const call = service.call as unknown as InputSchema;
const tell = service.tell as unknown as InputSchema;
const on = service.on as unknown as EventSchema;

describe('host:service contract', () => {
  test('call accepts a valid request and keeps the route opaque', () => {
    const parsed = call.schema?.parseInput?.({
      route: 'echo.EchoService.Echo',
      payloadBase64: 'AAEC',
      timeoutMs: 30000,
    });
    expect(parsed).toEqual({
      route: 'echo.EchoService.Echo',
      payloadBase64: 'AAEC',
      timeoutMs: 30000,
    });
  });

  test('call rejects a missing or empty route', () => {
    expect(() => call.schema?.parseInput?.({ payloadBase64: '' })).toThrow();
    expect(() =>
      call.schema?.parseInput?.({ route: '', payloadBase64: '' })
    ).toThrow();
  });

  test('call rejects invalid base64 payloads', () => {
    expect(() =>
      call.schema?.parseInput?.({ route: 'echo.EchoService.Echo', payloadBase64: 'AAE' })
    ).toThrow();
    expect(() =>
      call.schema?.parseInput?.({ route: 'echo.EchoService.Echo', payloadBase64: '!!!' })
    ).toThrow();
    expect(() =>
      call.schema?.parseInput?.({ route: 'echo.EchoService.Echo', payloadBase64: 'AB=' })
    ).toThrow();
    expect(() =>
      call.schema?.parseInput?.({ route: 'echo.EchoService.Echo', payloadBase64: 'A===' })
    ).toThrow();
  });

  test('call rejects a non-string payload', () => {
    expect(() =>
      call.schema?.parseInput?.({ route: 'echo.EchoService.Echo', payloadBase64: 1 })
    ).toThrow();
  });

  test('call rejects a non-positive timeout', () => {
    expect(() =>
      call.schema?.parseInput?.({
        route: 'echo.EchoService.Echo',
        payloadBase64: '',
        timeoutMs: 0,
      })
    ).toThrow();
  });

  test('call rejects non-finite timeouts', () => {
    expect(() =>
      call.schema?.parseInput?.({
        route: 'echo.EchoService.Echo',
        payloadBase64: '',
        timeoutMs: Number.NaN,
      })
    ).toThrow();
    expect(() =>
      call.schema?.parseInput?.({
        route: 'echo.EchoService.Echo',
        payloadBase64: '',
        timeoutMs: Number.POSITIVE_INFINITY,
      })
    ).toThrow();
  });

  test('call rejects timeouts above the host maximum', () => {
    expect(() =>
      call.schema?.parseInput?.({
        route: 'echo.EchoService.Echo',
        payloadBase64: '',
        timeoutMs: 300_001,
      })
    ).toThrow();
  });

  test('call validates the response payload', () => {
    expect(call.schema?.parseOutput?.({ payloadBase64: 'AAEC' })).toEqual({
      payloadBase64: 'AAEC',
    });
    expect(() => call.schema?.parseOutput?.({ payloadBase64: 1 })).toThrow();
  });

  test('tell rejects invalid base64 payloads', () => {
    expect(() =>
      tell.schema?.parseInput?.({ route: 'echo.EchoService.Echo', payloadBase64: 'AAE' })
    ).toThrow();
  });

  test('on rejects invalid base64 event payloads', () => {
    expect(() =>
      on.schema?.parseEvent?.({ event: 'echo.changed', payloadBase64: 'AAE' })
    ).toThrow();
  });

  test('call output rejects invalid base64 payloads', () => {
    expect(() => call.schema?.parseOutput?.({ payloadBase64: 'AAE' })).toThrow();
  });

  test('tell accepts a valid request and rejects a missing route', () => {
    expect(
      tell.schema?.parseInput?.({ route: 'echo.EchoService.Echo', payloadBase64: '' })
    ).toEqual({ route: 'echo.EchoService.Echo', payloadBase64: '' });
    expect(() => tell.schema?.parseInput?.({ payloadBase64: '' })).toThrow();
  });

  test('on validates the event name and payload', () => {
    expect(on.schema?.parseEvent?.({ event: 'echo.changed', payloadBase64: '' })).toEqual({
      event: 'echo.changed',
      payloadBase64: '',
    });
    expect(() => on.schema?.parseEvent?.({ event: '', payloadBase64: '' })).toThrow();
  });
});
