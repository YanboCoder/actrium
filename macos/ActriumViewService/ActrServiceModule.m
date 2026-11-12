//
//  ActrServiceModule.m
//  ActriumViewService
//
//  Objective-C bridge declaration for the Swift `ActrServiceModule`.
//  The implementation lives in ActrServiceModule.swift.
//

#import <React/RCTBridgeModule.h>

@interface RCT_EXTERN_MODULE(ActrServiceModule, NSObject)

RCT_EXTERN_METHOD(call:(NSString *)tabId
                  route:(NSString *)route
                  payloadBase64:(NSString *)payloadBase64
                  timeoutMs:(double)timeoutMs
                  resolve:(RCTPromiseResolveBlock)resolve
                  reject:(RCTPromiseRejectBlock)reject)

RCT_EXTERN_METHOD(tell:(NSString *)tabId
                  route:(NSString *)route
                  payloadBase64:(NSString *)payloadBase64
                  resolve:(RCTPromiseResolveBlock)resolve
                  reject:(RCTPromiseRejectBlock)reject)

@end
