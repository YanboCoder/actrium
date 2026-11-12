import React from 'react';
import {
  ActivityIndicator as NativeActivityIndicator,
  Image as NativeImage,
  NativeModules,
  Pressable as NativePressable,
  Text as NativeText,
  View as NativeView,
} from 'react-native';
import { EventHandler, HostHttpHandler } from 'askit/host-events';
import { Engine, type ComponentMap } from 'keel/host';
import {
  FlatList,
  ScrollView,
  Switch,
  Text,
  TextInput,
  View,
} from 'keel/host/preset';

import { hasDeclaredCapability } from './capabilities';
import { contract } from './contract';
import { ActrServiceError, actrServiceClient } from './host-service';
import {
  createHostModules,
  type ActorCall,
  type ActorTell,
} from './host-modules';
import { isAllowedImageSource, pickGuestImageProps } from './image-asset';
import { verifyRuntimeCapabilityManifestJSON } from './runtime-capability-verify';
import type { TabConfig } from './types';

type GuestButtonProps = {
  title: string;
  onPress?: () => void;
  color?: string;
  disabled?: boolean;
};

const GuestButton: React.FC<GuestButtonProps> = ({
  title,
  onPress,
  color = '#2563eb',
  disabled = false,
}) => (
  <NativePressable
    accessibilityRole='button'
    disabled={disabled}
    onPress={onPress}
    style={({ pressed }) => ({
      alignItems: 'center',
      backgroundColor: disabled ? '#31527f' : color,
      borderRadius: 10,
      opacity: pressed ? 0.8 : 1,
      padding: 14,
    })}
  >
    <NativeText style={{ color: '#ffffff', fontSize: 16, fontWeight: '600' }}>
      {title}
    </NativeText>
  </NativePressable>
);

const GuestImage: React.FC<React.ComponentProps<typeof NativeImage>> = (
  props
) => {
  if (!isAllowedImageSource(props.source)) {
    return null;
  }
  return (
    <NativeImage
      {...(pickGuestImageProps(props as Record<string, unknown>) as React.ComponentProps<typeof NativeImage>)}
    />
  );
};

const hostComponents: ComponentMap = {
  Button: GuestButton as unknown as React.ComponentType<
    Record<string, unknown>
  >,
  FlatList: FlatList as unknown as React.ComponentType<Record<string, unknown>>,
  // v1 exposes images as `ImageAsset`; the guest SDK still emits `Image`.
  Image: GuestImage as unknown as React.ComponentType<Record<string, unknown>>,
  ImageAsset: GuestImage as unknown as React.ComponentType<
    Record<string, unknown>
  >,
  ScrollView: ScrollView as React.ComponentType<Record<string, unknown>>,
  Switch: Switch as unknown as React.ComponentType<Record<string, unknown>>,
  Text: Text as React.ComponentType<Record<string, unknown>>,
  TextInput: TextInput as React.ComponentType<Record<string, unknown>>,
  View: View as React.ComponentType<Record<string, unknown>>,
};

type ActorCallResult = {
  success: boolean;
  payloadBase64?: string;
  error?: string;
};

type Props = {
  tab: TabConfig;
  containerWidth?: number;
  containerHeight?: number;
};

const logToNative = (message: string) => {
  console.log(message);
  try {
    NativeModules.NativeLogger?.log?.(message);
  } catch {
    // Logging must never affect guest execution.
  }
};

export const StandaloneGuestApp: React.FC<Props> = ({
  tab,
  containerWidth,
  containerHeight,
}) => {
  const [content, setContent] = React.useState<React.ReactElement | null>(null);
  const [error, setError] = React.useState('');
  const guestScript = tab.unifiedScript || tab.guestScript;
  const hasServiceCapability = hasDeclaredCapability(
    tab.permissions ?? [],
    'service'
  );
  const permissionsKey = [...(tab.permissions ?? [])].sort().join(',');

  React.useEffect(() => {
    // A reload must start from a clean state, otherwise a previous load
    // failure keeps rendering even after the new bundle loads successfully.
    setError('');
    setContent(null);

    if (!guestScript) {
      setError('The application does not contain a Keel guest bundle.');
      return;
    }
    const capabilityManifestJSON = tab.capabilityManifestJSON;
    if (!capabilityManifestJSON) {
      setError('The package does not contain a capability manifest.');
      return;
    }
    const verification = verifyRuntimeCapabilityManifestJSON(
      capabilityManifestJSON,
      tab.permissions ?? []
    );
    if (!verification.ok) {
      const details = verification.details.join(' ');
      logToNative(
        `[StandaloneGuestApp] capability verification failed: ${details}`
      );
      setError(`Capability manifest verification failed: ${details}`);
      return;
    }

    let disposed = false;
    const callLegacyActor: ActorCall = async ({
      requestId,
      route,
      payloadBase64,
      timeoutMs,
    }) => {
      const gatewayUrl =
        globalThis.__ACTRIUM_ACTR_GATEWAY_URL__ ??
        'http://127.0.0.1:8391/v1/call';
      logToNative(`[StandaloneGuestApp] legacy actor call ${route}`);
      const result = await HostHttpHandler.handleRequest<ActorCallResult>({
        requestId,
        url: gatewayUrl,
        method: 'POST',
        body: { routeKey: route, payloadBase64, timeoutMs },
      });
      return {
        success: result.success && result.data?.success === true,
        payloadBase64: result.data?.payloadBase64,
        message: result.data?.error ?? result.message,
      };
    };
    // Route through the shared host:service client: it prefers the per-tab
    // native module (Renderer → Host IPC) and falls back to the tab's
    // `serviceCallUrl`. With neither available it fails closed.
    const callServiceActor: ActorCall = async ({
      route,
      payloadBase64,
      timeoutMs,
    }) => {
      try {
        const result = await actrServiceClient.call(
          { route, payloadBase64, timeoutMs },
          { tabId: tab.id, serviceCallUrl: tab.serviceCallUrl }
        );
        return { success: true, payloadBase64: result.payloadBase64 };
      } catch (cause) {
        const code =
          cause instanceof ActrServiceError ? cause.code : 'INTERNAL';
        const message = cause instanceof Error ? cause.message : String(cause);
        logToNative(`[StandaloneGuestApp] service call failed ${code}: ${message}`);
        return { success: false, message: `${code}: ${message}` };
      }
    };
    const tellServiceActor: ActorTell = async ({ route, payloadBase64 }) => {
      try {
        await actrServiceClient.tell(
          { route, payloadBase64 },
          { tabId: tab.id, serviceCallUrl: tab.serviceCallUrl }
        );
        return { success: true };
      } catch (cause) {
        const code =
          cause instanceof ActrServiceError ? cause.code : 'INTERNAL';
        const message = cause instanceof Error ? cause.message : String(cause);
        return { success: false, message: `${code}: ${message}` };
      }
    };
    const hostModules = createHostModules({
      hasServiceCapability,
      callActor: callServiceActor,
      tellActor: tellServiceActor,
    });
    const engine = new Engine({ debug: false, contract, hostModules });
    engine.register(hostComponents);
    engine.createReceiver();

    const update = () => {
      if (disposed) return;
      const next = engine.getReceiver()?.render();
      setContent(next as React.ReactElement | null);
    };
    const unsubscribeUpdate = engine.on('update', update);
    // The legacy bridge is confined to the host-designated demo package. The
    // designation comes from launch configuration, never from package data.
    // Every other package must use the per-tab host:service path.
    const legacyEchoTab = tab.legacyActorBridge === true;
    const unsubscribeEvents = legacyEchoTab
      ? EventHandler.setup(engine, {
      tabId: tab.id,
      onLog: (direction, event) => {
        logToNative(`[StandaloneGuestApp] ${direction}: ${event}`);
      },
      handlers: {
        ACTR_CALL: {
          responseEvent: 'ACTR_CALL_RESULT',
          handle: async (payload: {
            requestId?: string;
            routeKey?: unknown;
            payloadBase64?: unknown;
            timeoutMs?: unknown;
          }) => {
            const requestId = payload?.requestId;
            if (!hasServiceCapability) {
              return {
                requestId,
                success: false,
                error:
                  'The application does not declare the service capability.',
              };
            }
            if (
              typeof payload?.routeKey !== 'string' ||
              typeof payload?.payloadBase64 !== 'string'
            ) {
              return {
                requestId,
                success: false,
                error: 'ACTR_CALL requires routeKey and payloadBase64 strings.',
              };
            }

            logToNative(`[StandaloneGuestApp] ACTR_CALL ${payload.routeKey}`);
            const response = await callLegacyActor({
              requestId: requestId ?? `actr-${Date.now()}`,
              route: payload.routeKey,
              payloadBase64: payload.payloadBase64,
              timeoutMs:
                typeof payload.timeoutMs === 'number'
                  ? payload.timeoutMs
                  : 30_000,
            });
            logToNative(
              `[StandaloneGuestApp] ACTR_CALL completed: ${response.success ? 'success' : 'failure'}`,
            );
            return {
              requestId,
              success: response.success,
              payloadBase64: response.payloadBase64,
              error: response.message,
            };
          },
        },
      },
    })
      : () => {};

    void engine
      .loadBundle(guestScript, { containerHeight, containerWidth })
      .then(() => {
        const stats = engine.getReceiver()?.getStats();
        logToNative(
          `[StandaloneGuestApp] guest loaded: roots=${stats?.rootChildrenCount ?? 0}, nodes=${stats?.nodeCount ?? 0}`,
        );
        update();
        setTimeout(update, 0);
      })
      .catch((cause) => {
        if (!disposed) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      });

    return () => {
      disposed = true;
      unsubscribeEvents();
      unsubscribeUpdate();
      engine.destroy();
    };
  }, [
    // Do not recreate the engine on window resize. The first layout size is
    // enough for the guest bundle, and RN handles later size changes.
    // reloadKey still forces a clean reload when the package changes.
    guestScript,
    hasServiceCapability,
    tab.id,
    tab.legacyActorBridge,
    tab.serviceCallUrl,
    tab.reloadKey,
    tab.capabilityManifestJSON,
    permissionsKey,
  ]);

  if (error) {
    return (
      <NativeView
        style={{ flex: 1, padding: 24, backgroundColor: '#2b0b0b' }}
      >
        <NativeText style={{ color: '#fda4af' }}>{error}</NativeText>
      </NativeView>
    );
  }

  return (
    <NativeView collapsable={false} style={{ flex: 1 }}>
      {content ?? (
        <NativeView
          style={{
            flex: 1,
            alignItems: 'center',
            justifyContent: 'center',
            backgroundColor: '#101827',
          }}
        >
          <NativeActivityIndicator color='#60a5fa' />
        </NativeView>
      )}
    </NativeView>
  );
};
