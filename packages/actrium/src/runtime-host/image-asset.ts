/**
 * Only package-local asset ids can be rendered without a network capability.
 * Remote URIs would let a Guest bypass the host:net broker and attach
 * arbitrary headers, so they stay rejected until image loading is routed
 * through that broker.
 */
export function isAllowedImageSource(source: unknown): boolean {
  return typeof source === 'number';
}

/**
 * Display-only props forwarded to the native image. Network-affecting props
 * such as `src`, `srcSet`, `crossOrigin`, `referrerPolicy`, `defaultSource`
 * and `loadingIndicatorSource` are deliberately excluded so a Guest cannot
 * bypass the `source` check above.
 */
export const ALLOWED_GUEST_IMAGE_PROPS = [
  'source',
  'style',
  'resizeMode',
  'testID',
  'accessible',
  'accessibilityLabel',
  'onLoad',
  'onError',
] as const;

export function pickGuestImageProps(
  props: Record<string, unknown>
): Record<string, unknown> {
  const picked: Record<string, unknown> = {};
  for (const key of ALLOWED_GUEST_IMAGE_PROPS) {
    if (props[key] !== undefined) {
      picked[key] = props[key];
    }
  }
  return picked;
}
