/**
 * Wire types for the `host:service` Keel host module.
 *
 * These are the untrusted values that cross the guest/host boundary, so every
 * field is a primitive. Binary payloads travel base64-encoded, matching the
 * existing ACTR_CALL event shape; a later revision can move to the contract's
 * `binary` metadata without changing the guest-facing call site.
 */

export type ServiceCallInput = {
  route: string;
  payloadBase64: string;
  timeoutMs?: number;
};

export type ServiceCallOutput = {
  payloadBase64: string;
};

export type ServiceTellInput = {
  route: string;
  payloadBase64: string;
};

/** Stable transport error codes surfaced to the guest as `ActrError`. */
export type ActrServiceErrorCode =
  | 'TRANSPORT_ERROR'
  | 'INVALID_RESPONSE'
  | 'SERVICE_NOT_FOUND'
  | 'TIMED_OUT'
  | 'INTERNAL';

export class ActrServiceError extends Error {
  readonly code: ActrServiceErrorCode;

  constructor(code: ActrServiceErrorCode, message: string) {
    // Reason: guests match on `name`/`code`, not on the message.
    super(message);
    this.name = 'ActrError';
    this.code = code;
  }
}
