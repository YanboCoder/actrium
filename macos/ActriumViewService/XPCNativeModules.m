//
//  XPCNativeModules.m
//  ActriumViewService
//
//  Objective-C bridge for native modules in XPC service.
//  Uses RCT_EXTERN_MODULE to expose Swift modules to React Native.
//

#import <React/RCTBridgeModule.h>
#import <objc/message.h>

// MARK: - RCTHost Event Emitter Helper (Bridgeless Mode)

/// Helper function to call RCTHost's callFunctionOnJSModule:method:args:
/// Swift's performSelector only supports up to 2 arguments, so we use this ObjC helper.
/// @param host The RCTHost instance (obtained from rootViewFactory.reactHost)
/// @param module The JS module name (e.g., "RCTDeviceEventEmitter")
/// @param method The method name (e.g., "emit")
/// @param args The arguments array
void RCTHostCallFunction(id host, NSString *module, NSString *method, NSArray *args) {
    if (!host) return;
    SEL sel = NSSelectorFromString(@"callFunctionOnJSModule:method:args:");
    if ([host respondsToSelector:sel]) {
        typedef void (*MsgSendFunc)(id, SEL, NSString *, NSString *, NSArray *);
        MsgSendFunc func = (MsgSendFunc)objc_msgSend;
        func(host, sel, module, method, args);
    }
}

// MARK: - Capability Permission

@interface RCT_EXTERN_MODULE(RealmService, NSObject)

RCT_EXTERN_METHOD(call:(NSString *)tabId
                  route:(NSString *)route
                  payloadBase64:(NSString *)payloadBase64
                  timeoutMs:(NSNumber *)timeoutMs
                  resolver:(RCTPromiseResolveBlock)resolve
                  rejecter:(RCTPromiseRejectBlock)reject)

@end

@interface RCT_EXTERN_MODULE(CapabilityPermission, NSObject)

RCT_EXTERN_METHOD(requestGrant:(NSString *)tabId capability:(NSString *)capability)

@end

// MARK: - StartPageModule

@interface RCT_EXTERN_MODULE(StartPageModule, NSObject)

RCT_EXTERN_METHOD(openURL:(NSString *)urlString)

@end

// MARK: - HistoryModule

@interface RCT_EXTERN_MODULE(HistoryModule, NSObject)

RCT_EXTERN_METHOD(openHistoryItem:(NSString *)itemId :(NSString *)appURL)
RCT_EXTERN_METHOD(deleteHistoryItem:(NSString *)itemId)
RCT_EXTERN_METHOD(restoreGroup:(NSString *)groupId)
RCT_EXTERN_METHOD(clearHistory)

@end

// MARK: - BookmarksModule

@interface RCT_EXTERN_MODULE(BookmarksModule, NSObject)

RCT_EXTERN_METHOD(openBookmark:(NSString *)bookmarkId :(NSString *)appURL)
RCT_EXTERN_METHOD(deleteBookmark:(NSString *)bookmarkId)

@end

// MARK: - NativeLogger

@interface RCT_EXTERN_MODULE(NativeLogger, NSObject)

RCT_EXTERN_METHOD(log:(NSString *)message)
RCT_EXTERN_METHOD(warn:(NSString *)message)
RCT_EXTERN_METHOD(error:(NSString *)message)

@end
