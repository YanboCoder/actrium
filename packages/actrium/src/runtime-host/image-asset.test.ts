import { describe, expect, test } from 'bun:test';
import { isAllowedImageSource, pickGuestImageProps } from './image-asset';

describe('isAllowedImageSource', () => {
  test('allows package asset ids', () => {
    expect(isAllowedImageSource(42)).toBe(true);
  });

  test('rejects remote and header-bearing sources', () => {
    expect(isAllowedImageSource('https://example.com/image.png')).toBe(false);
    expect(isAllowedImageSource({ uri: 'https://example.com/image.png' })).toBe(
      false
    );
    expect(
      isAllowedImageSource({
        uri: 'https://example.com/image.png',
        headers: { Authorization: 'secret' },
      })
    ).toBe(false);
    expect(isAllowedImageSource(undefined)).toBe(false);
  });

  test('forwards only display props', () => {
    expect(
      pickGuestImageProps({
        source: 42,
        style: { width: 10 },
        testID: 'guest-image',
        src: 'https://example.com/image.png',
        srcSet: 'https://example.com/image.png 2x',
        crossOrigin: 'use-credentials',
        referrerPolicy: 'no-referrer-when-downgrade',
        defaultSource: { uri: 'https://example.com/default.png' },
        loadingIndicatorSource: {
          uri: 'https://example.com/loading.png',
        },
      })
    ).toEqual({
      source: 42,
      style: { width: 10 },
      testID: 'guest-image',
    });
  });
});
