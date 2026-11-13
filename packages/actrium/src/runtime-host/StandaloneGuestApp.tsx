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
import {
  capabilitiesFromSnapshotJSON,
  CapabilityAuthorizer,
} from './capability-authorization';
import { contract } from './contract';
import { createHostModules, type ActorCall } from './host-modules';
import { isAllowedImageSource, pickGuestImageProps } from './image-asset';
import { verifyRuntimeCapabilityManifestJSON } from './runtime-capability-verify';
import { bindV1HostModules, verifyV1RuntimeGuest } from './v1-guest';
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

/**
 * RN's Clipboard JS wrapper is deprecated; call the native module directly.
 * RCTClipboard is part of react-native-macos and exports `setString`.
 */
const writeClipboardText = async (text: string): Promise<void> => {
  const clipboard = NativeModules.Clipboard as
    | { setString?: (value: string) => void }
    | undefined;
  if (!clipboard?.setString) {
    throw new Error('The host clipboard module is not available.');
  }
  clipboard.setString(text);
};

type CapabilityPermissionBridge = {
  requestGrant?: (tabId: string, capability: string) => void;
};

const requestNativeCapabilityGrant = (
  tabId: string,
  capability: string
): void => {
  const bridge = NativeModules.CapabilityPermission as
    | CapabilityPermissionBridge
    | undefined;
  if (!bridge?.requestGrant) {
    throw new Error('The native capability permission bridge is not available.');
  }
  bridge.requestGrant(tabId, capability);
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
    'service.call'
  );
  const permissionsKey = [...(tab.permissions ?? [])].sort().join(',');
  const capabilitySnapshot = React.useMemo(
    () =>
      tab.capabilitySnapshotJSON
        ? capabilitiesFromSnapshotJSON(tab.capabilitySnapshotJSON)
        : null,
    [tab.capabilitySnapshotJSON]
  );
  const capabilitySnapshotAvailable = capabilitySnapshot !== null;
  const authorizerRef = React.useRef<CapabilityAuthorizer | null>(null);
  if (!authorizerRef.current && capabilitySnapshot) {
    authorizerRef.current = new CapabilityAuthorizer(
      capabilitySnapshot,
      (capability) => requestNativeCapabilityGrant(tab.id, capability)
    );
  }

  React.useEffect(() => {
    if (capabilitySnapshot) {
      authorizerRef.current?.update(capabilitySnapshot);
    }
  }, [capabilitySnapshot]);

  React.useEffect(
    () => () => {
      authorizerRef.current?.dispose();
    },
    [tab.id]
  );

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
    // Only the Host-designated Echo demo may use the earlier Guest bundle format.
    const legacyEchoTab = tab.legacyActorBridge === true;
    const v1 = legacyEchoTab
      ? undefined
      : verifyV1RuntimeGuest(guestScript, capabilityManifestJSON, tab.permissions ?? []);
    const verification = v1 ?? verifyRuntimeCapabilityManifestJSON(
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
    const authorizer = authorizerRef.current;
    if (!authorizer) {
      setError('The Native capability authorization snapshot is unavailable.');
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
    const callServiceActor: ActorCall = async ({
      requestId,
      route,
      payloadBase64,
      timeoutMs,
    }) => {
      // The global echo gateway is legacy-bridge only. New service calls must
      // target the tab's attached Service actor.
      if (!tab.serviceCallUrl) {
        const service = NativeModules.RealmService;
        if (!service?.call) {
          return { success: false, message: 'The native Realm Service bridge is unavailable.' };
        }
        try {
          const { payloadBase64: response } = await service.call(
            tab.id,
            route,
            payloadBase64,
            timeoutMs
          );
          return { success: true, payloadBase64: response };
        } catch (error) {
          return { success: false, message: String(error) };
        }
      }
      logToNative(`[StandaloneGuestApp] service call ${route}`);
      const result = await HostHttpHandler.handleRequest<ActorCallResult>({
        requestId,
        url: tab.serviceCallUrl,
        method: 'POST',
        body: { routeKey: route, payloadBase64, timeoutMs },
      });
      return {
        success: result.success && result.data?.success === true,
        payloadBase64: result.data?.payloadBase64,
        message: result.data?.error ?? result.message,
      };
    };
    const hostModules = createHostModules({
      hasServiceCapability,
      callActor: callServiceActor,
      authorizeCapability: (capability) => authorizer.authorize(capability),
      writeClipboardText,
    });
    // Expose exactly the verified method and component set to this Guest Engine.
    const linked = v1?.ok ? v1 : undefined;
    const engine = linked
      ? new Engine({
          debug: false,
          ...bindV1HostModules(linked.linkage, hostModules, (capability) =>
            authorizer.authorize(capability)
          ),
          requireWhitelist: linked.manifest.imports.filter((id) => !id.startsWith('host:')),
          restrictToRegisteredComponents: true,
          // Implicit text needs a renderer even when Text is absent from the manifest.
          textNodeRenderer: Text,
        })
      : new Engine({ debug: false, contract, hostModules });
    engine.register(
      linked
        ? Object.fromEntries(
            Object.entries(hostComponents).filter(([name]) =>
              linked.manifest.components.includes(name === 'Image' ? 'ImageAsset' : name)
            )
          )
        : hostComponents
    );
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
    capabilitySnapshotAvailable,
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
