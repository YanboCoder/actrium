#import "JSCSandboxJSI.h"
#import <Foundation/Foundation.h>
#import <JavaScriptCore/JavaScriptCore.h>
#import <dlfcn.h>
#import <os/log.h>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>
#include <utility>
#include <vector>

namespace jsc_sandbox {

// MARK: - Execution time limit (private JSC API, resolved via dlsym)
//
// JavaScriptCore's ONLY mechanism to interrupt running JS is
// JSContextGroupSetExecutionTimeLimit / JSContextGroupClearExecutionTimeLimit,
// declared in the PRIVATE header JSContextRefPrivate.h. Referencing them
// statically would embed private symbols in the binary and trip App Store
// review, so they are looked up at runtime with dlsym(RTLD_DEFAULT, ...) and
// only used when the embedder explicitly opts in via
// createRuntime({ enableExecutionTimeLimit: true }). Default builds never
// touch the symbols beyond the (legal) dlsym probe guarded by that flag.
//
// Function pointer types mirror WebKit's JSContextRefPrivate.h:
//   typedef bool (*JSShouldTerminateCallback)(JSContextRef ctx, void* context);
//   JS_EXPORT void JSContextGroupSetExecutionTimeLimit(
//       JSContextGroupRef group, double limit /* SECONDS */,
//       JSShouldTerminateCallback callback, void* context);
//   JS_EXPORT void JSContextGroupClearExecutionTimeLimit(JSContextGroupRef);
// Note the limit is in SECONDS (double), while our public option is in ms.

namespace {

// Byte-owning jsi::MutableBuffer used to rebuild a HOST-realm ArrayBuffer
// from bytes copied out of the sandbox. A copy is mandatory: the sandbox
// value's backing store dies with the sandbox GC, and the two realms must
// never alias each other's memory.
class VectorBuffer : public jsi::MutableBuffer {
public:
  VectorBuffer(const uint8_t *data, size_t size) : bytes_(data, data + size) {}
  size_t size() const override { return bytes_.size(); }
  uint8_t *data() override { return bytes_.data(); }

private:
  std::vector<uint8_t> bytes_;
};

// Copy `size` bytes into a fresh host-realm ArrayBuffer. Throws (loudly) if
// the host runtime does not implement createArrayBuffer — never a silent
// empty object.
jsi::Value makeHostArrayBuffer(jsi::Runtime &rt, const uint8_t *data,
                               size_t size) {
  return jsi::ArrayBuffer(rt, std::make_shared<VectorBuffer>(data, size));
}

// Host-realm constructor name for a JSC typed-array kind; nullptr when the
// kind has no same-name reconstruction (the caller falls back to the raw
// ArrayBuffer of the view's byte window).
const char *typedArrayCtorName(JSTypedArrayType type) {
  switch (type) {
  case kJSTypedArrayTypeInt8Array:
    return "Int8Array";
  case kJSTypedArrayTypeInt16Array:
    return "Int16Array";
  case kJSTypedArrayTypeInt32Array:
    return "Int32Array";
  case kJSTypedArrayTypeUint8Array:
    return "Uint8Array";
  case kJSTypedArrayTypeUint8ClampedArray:
    return "Uint8ClampedArray";
  case kJSTypedArrayTypeUint16Array:
    return "Uint16Array";
  case kJSTypedArrayTypeUint32Array:
    return "Uint32Array";
  case kJSTypedArrayTypeFloat32Array:
    return "Float32Array";
  case kJSTypedArrayTypeFloat64Array:
    return "Float64Array";
  default:
    return nullptr;
  }
}

// Inverse of typedArrayCtorName: the JSC typed-array kind for a host view
// constructor name, or kJSTypedArrayTypeNone when JSC has no matching kind
// (BigInt64Array / BigUint64Array / DataView — the caller falls back to the
// raw ArrayBuffer, so the bytes still cross).
JSTypedArrayType typedArrayTypeForCtorName(const std::string &name) {
  if (name == "Int8Array") return kJSTypedArrayTypeInt8Array;
  if (name == "Uint8Array") return kJSTypedArrayTypeUint8Array;
  if (name == "Uint8ClampedArray") return kJSTypedArrayTypeUint8ClampedArray;
  if (name == "Int16Array") return kJSTypedArrayTypeInt16Array;
  if (name == "Uint16Array") return kJSTypedArrayTypeUint16Array;
  if (name == "Int32Array") return kJSTypedArrayTypeInt32Array;
  if (name == "Uint32Array") return kJSTypedArrayTypeUint32Array;
  if (name == "Float32Array") return kJSTypedArrayTypeFloat32Array;
  if (name == "Float64Array") return kJSTypedArrayTypeFloat64Array;
  return kJSTypedArrayTypeNone;
}

// Host-realm ArrayBufferView constructor names eligible for host->sandbox
// same-kind reconstruction (also the shape-detection allowlist).
bool isViewCtorName(const std::string &name) {
  static const char *kNames[] = {
      "Int8Array",    "Uint8Array",    "Uint8ClampedArray", "Int16Array",
      "Uint16Array",  "Int32Array",    "Uint32Array",       "Float32Array",
      "Float64Array", "BigInt64Array", "BigUint64Array",    "DataView"};
  for (const char *candidate : kNames) {
    if (name == candidate) {
      return true;
    }
  }
  return false;
}

// Extract the byte WINDOW + view constructor name from a HOST jsi::Object that
// is an ArrayBufferView shape ({buffer: ArrayBuffer, byteOffset, byteLength}).
// Returns false when `obj` is not view-shaped or the window is out of range.
// byteOffset/byteLength are validated as doubles BEFORE narrowing to size_t
// (NaN/Infinity/negative/non-integer rejected): a merely view-shaped plain
// object could otherwise force a UB / implementation-defined cast.
bool extractHostViewBytes(jsi::Runtime &hostRt, const jsi::Object &obj,
                          std::vector<uint8_t> &out, std::string &ctorName) {
  jsi::Value ctorVal = obj.getProperty(hostRt, "constructor");
  if (!ctorVal.isObject()) {
    return false;
  }
  jsi::Value nameVal = ctorVal.getObject(hostRt).getProperty(hostRt, "name");
  if (!nameVal.isString()) {
    return false;
  }
  std::string name = nameVal.getString(hostRt).utf8(hostRt);
  if (!isViewCtorName(name)) {
    return false;
  }

  jsi::Value bufferVal = obj.getProperty(hostRt, "buffer");
  jsi::Value offsetVal = obj.getProperty(hostRt, "byteOffset");
  jsi::Value lengthVal = obj.getProperty(hostRt, "byteLength");
  if (!bufferVal.isObject() || !offsetVal.isNumber() || !lengthVal.isNumber()) {
    return false;
  }
  jsi::Object bufferObj = bufferVal.getObject(hostRt);
  if (!bufferObj.isArrayBuffer(hostRt)) {
    return false;
  }

  // Bound byteOffset/byteLength by the REAL backing size in the DOUBLE domain
  // before narrowing to size_t. backingSize is a true allocation (<< 2^53, so
  // exactly representable as double); this closes the 2^64-rounding hole of
  // static_cast<double>(SIZE_MAX) — where byteOffset == 2**64 slips past and
  // then casts out of range — and makes the narrowing below lossless.
  jsi::ArrayBuffer backing = bufferObj.getArrayBuffer(hostRt);
  const size_t backingSize = backing.size(hostRt);
  const double backingSizeD = static_cast<double>(backingSize);
  const double offsetD = offsetVal.getNumber();
  const double lengthD = lengthVal.getNumber();
  if (!std::isfinite(offsetD) || !std::isfinite(lengthD) || offsetD < 0 ||
      lengthD < 0 || offsetD != std::floor(offsetD) ||
      lengthD != std::floor(lengthD) || offsetD > backingSizeD ||
      lengthD > backingSizeD - offsetD) {
    return false;
  }

  const auto offset = static_cast<size_t>(offsetD);
  const auto length = static_cast<size_t>(lengthD);

  const uint8_t *base = backing.data(hostRt) + offset;
  out.assign(base, base + length);
  ctorName = name;
  return true;
}

// Build a sandbox-realm JSValue* from COPIED bytes: a raw ArrayBuffer when
// `viewCtorName` is nullptr, otherwise the same-kind typed-array view (falling
// back to the raw ArrayBuffer when JSC has no matching kind). The copy is owned
// by the new ArrayBuffer and freed by its deallocator on GC.
void *makeSandboxBytes(JSContext *ctx, const uint8_t *data, size_t len,
                       const char *viewCtorName) {
  JSContextRef ctxRef = [ctx JSGlobalContextRef];
  void *buf = malloc(len ? len : 1);
  if (len) {
    memcpy(buf, data, len);
  }
  JSValueRef exc = nullptr;
  JSObjectRef abRef = JSObjectMakeArrayBufferWithBytesNoCopy(
      ctxRef, buf, len, [](void *bytes, void *) { free(bytes); }, nullptr, &exc);
  if (exc || !abRef) {
    // Ownership is NOT transferred when creation fails — free it ourselves.
    free(buf);
    return (__bridge void *)[JSValue valueWithUndefinedInContext:ctx];
  }

  if (viewCtorName) {
    JSTypedArrayType type = typedArrayTypeForCtorName(viewCtorName);
    if (type != kJSTypedArrayTypeNone) {
      JSValueRef viewExc = nullptr;
      JSObjectRef viewRef =
          JSObjectMakeTypedArrayWithArrayBuffer(ctxRef, type, abRef, &viewExc);
      if (!viewExc && viewRef) {
        return (__bridge void *)[JSValue valueWithJSValueRef:viewRef
                                                   inContext:ctx];
      }
    }
    // Unknown/unsupported kind (or wrap failure): fall back to the ArrayBuffer.
  }
  return (__bridge void *)[JSValue valueWithJSValueRef:abRef inContext:ctx];
}

using JSCShouldTerminateCallback = bool (*)(JSContextRef ctx, void *context);
using JSCSetExecutionTimeLimitFn = void (*)(JSContextGroupRef group,
                                            double limitSeconds,
                                            JSCShouldTerminateCallback callback,
                                            void *context);
using JSCClearExecutionTimeLimitFn = void (*)(JSContextGroupRef group);

struct TimeLimitAPI {
  JSCSetExecutionTimeLimitFn set = nullptr;
  JSCClearExecutionTimeLimitFn clear = nullptr;
};

// Resolve both symbols once, process-wide. Either both resolve or the API is
// treated as unavailable — we must never arm a limit we cannot clear.
const TimeLimitAPI &timeLimitAPI() {
  static const TimeLimitAPI api = [] {
    TimeLimitAPI a;
    a.set = reinterpret_cast<JSCSetExecutionTimeLimitFn>(
        dlsym(RTLD_DEFAULT, "JSContextGroupSetExecutionTimeLimit"));
    a.clear = reinterpret_cast<JSCClearExecutionTimeLimitFn>(
        dlsym(RTLD_DEFAULT, "JSContextGroupClearExecutionTimeLimit"));
    if (!a.set || !a.clear) {
      a.set = nullptr;
      a.clear = nullptr;
    }
    return a;
  }();
  return api;
}

// Invoked by JSC on the JS-executing thread once the armed limit expires.
// Returning true terminates execution (script throws an uncatchable
// termination exception); `context` is the owning JSCSandboxContext's
// timeLimitFired_ flag.
bool timeLimitShouldTerminate(JSContextRef ctx, void *context) {
  (void)ctx;
  auto *fired = static_cast<std::atomic<bool> *>(context);
  fired->store(true, std::memory_order_release);
  return true;
}

} // namespace

// Safe extraction of error message from a JSValue without triggering
// toString recursion. [JSValue toString] executes JS code which can throw,
// re-entering the exception handler and causing stack overflow (SIGBUS).
static NSString *safeExceptionMessage(JSValue *exception) {
  if (!exception) return @"(null exception)";
  if ([exception isString]) {
    return [exception toString];
  }
  // For Error objects, .message is typically a plain string
  JSValue *msg = exception[@"message"];
  if (msg && [msg isString]) {
    JSValue *name = exception[@"name"];
    NSString *nameStr = (name && [name isString]) ? [name toString] : @"Error";
    return [NSString stringWithFormat:@"%@: %@", nameStr, [msg toString]];
  }
  return @"(exception: cannot safely stringify)";
}

// MARK: - JSCSandboxContext Implementation

struct JSCSandboxContext::ConversionState {
  // Host objects on the current conversion path, paired with the guest value
  // already materialized for them. Identity is compared with
  // Runtime::strictEquals, so only the same host object matches. Entries are
  // removed when a subtree completes, mirroring the guest->host converter:
  // shared (non-cyclic) references stay duplicated, only ancestor cycles are
  // resolved to the same guest object.
  std::vector<std::pair<jsi::Object, void *>> objects;
};

struct JSCSandboxContext::ConversionScope {
  ConversionState &state;
  ConversionScope(ConversionState &s, jsi::Runtime &rt, const jsi::Object &obj,
                  void *guest)
      : state(s) {
    state.objects.emplace_back(jsi::Value(rt, obj).asObject(rt), guest);
  }
  ~ConversionScope() { state.objects.pop_back(); }
  ConversionScope(const ConversionScope &) = delete;
  ConversionScope &operator=(const ConversionScope &) = delete;
};

struct JSCSandboxContext::PromiseBridge {
  // Strong references to the guest resolving functions. Both are released as
  // soon as the Promise settles, so a long-lived sandbox does not root one
  // pair of callbacks per host Promise forever.
  JSValue *resolve = nil;
  JSValue *reject = nil;
  bool settled = false;
};

void *JSCSandboxContext::lookupConvertedObject(jsi::Runtime &rt,
                                               const jsi::Object &obj,
                                               ConversionState &state) {
  for (const auto &entry : state.objects) {
    if (jsi::Object::strictEquals(rt, entry.first, obj)) {
      return entry.second;
    }
  }
  return nullptr;
}

JSCSandboxContext::JSCSandboxContext(jsi::Runtime &hostRuntime,
                                     double timeoutMs,
                                     bool enableExecutionTimeLimit)
    : jsContext_(nullptr), contextGroup_(nullptr), hostRuntime_(&hostRuntime),
      timeoutMs_(timeoutMs), timeLimitEnabled_(false), timeLimitDepth_(0),
      timeLimitFired_(false), disposed_(false), errorFactory_(nullptr),
      promiseBridge_(nullptr) {
  // Execution timeout has TWO states:
  // - Default (enableExecutionTimeLimit == false): NOT ENFORCED.
  //   JavaScriptCore's public API has no way to interrupt running JS (no
  //   equivalent of QuickJS's JS_SetInterruptHandler), so the
  //   createRuntime({timeout}) option is accepted but ignored and a tenant
  //   infinite loop blocks the calling (host) thread indefinitely. Callers
  //   must not rely on this engine for CPU isolation in this state.
  // - Opt-in (enableExecutionTimeLimit == true): ENFORCED via the private
  //   JSContextGroupSetExecutionTimeLimit API resolved through dlsym (see
  //   timeLimitAPI() above). Each top-level eval / host->sandbox call gets a
  //   wall-clock budget of timeoutMs; on expiry JSC terminates the script and
  //   the call throws a clear timeout error while the context stays usable.
  //   If the private symbols cannot be resolved, this logs and falls back to
  //   the unenforced behavior above.
  @autoreleasepool {
    JSContext *ctx = [[JSContext alloc] init];
    if (!ctx) {
      throw jsi::JSError(hostRuntime, "Failed to create JSContext");
    }

    if (enableExecutionTimeLimit) {
      if (timeLimitAPI().set != nullptr) {
        // JSContextGetGroup is public API; the group is owned by the
        // context's VM, which we keep alive via jsContext_ until dispose().
        contextGroup_ =
            const_cast<void *>((const void *)JSContextGetGroup(
                [ctx JSGlobalContextRef]));
        timeLimitEnabled_ = (contextGroup_ != nullptr);
      }
      if (!timeLimitEnabled_) {
        // WARNING-level: the caller explicitly asked for enforcement but this
        // JSC build does not export the private API — timeouts will NOT be
        // enforced and a tenant loop can hang the host thread.
        os_log_error(OS_LOG_DEFAULT,
                     "[JSCSandbox] enableExecutionTimeLimit requested but "
                     "JSContextGroupSetExecutionTimeLimit is unavailable; "
                     "falling back to UNENFORCED timeouts");
      }
    }

    // Set up exception handler - must store exception for later checking.
    // CRITICAL: Use recursion guard to prevent infinite toString recursion.
    // [JSValue toString] executes JS code which can throw, re-entering this
    // handler and causing stack overflow (EXC_BAD_ACCESS / SIGBUS).
    __block BOOL inExceptionHandler = NO;
    ctx.exceptionHandler = ^(JSContext *context, JSValue *exception) {
      context.exception = exception; // Preserve for checking after eval
      if (inExceptionHandler) return; // Break recursion cycle
      inExceptionHandler = YES;
      NSLog(@"[JSCSandbox] Exception: %@", safeExceptionMessage(exception));
      inExceptionHandler = NO;
    };

    // Inject console shim
    // message is always a string from JS-side .join(' '), safe to toString.
    // Guard against non-string values to avoid toString recursion.
    JSValue *consoleLog = [JSValue
        valueWithObject:^(JSValue *message) {
          if ([message isString]) {
            NSLog(@"[JSCSandbox] %@", [message toString]);
          } else {
            NSLog(@"[JSCSandbox] [non-string value]");
          }
        }
              inContext:ctx];
    ctx[@"__jsc_console_log"] = consoleLog;

    NSString *consoleScript = @R"(
            var console = {
                log: function() { __jsc_console_log(Array.prototype.slice.call(arguments).join(' ')); },
                warn: function() { __jsc_console_log('[WARN] ' + Array.prototype.slice.call(arguments).join(' ')); },
                error: function() { __jsc_console_log('[ERROR] ' + Array.prototype.slice.call(arguments).join(' ')); },
                info: function() { __jsc_console_log('[INFO] ' + Array.prototype.slice.call(arguments).join(' ')); },
                debug: function() { __jsc_console_log('[DEBUG] ' + Array.prototype.slice.call(arguments).join(' ')); },
                assert: function(cond) { if (!cond) __jsc_console_log('[ASSERT] ' + Array.prototype.slice.call(arguments, 1).join(' ')); },
                trace: function() {},
                time: function() {},
                timeEnd: function() {},
                group: function() {},
                groupEnd: function() {}
            };
        )";
    [ctx evaluateScript:consoleScript];

    // Capture realm intrinsics while the realm is pristine. Tenant code can
    // replace globalThis.Error/Promise at any time; conversion and
    // failure-path helpers must not depend on those mutable globals.
    errorFactory_ = (__bridge_retained void *)[ctx evaluateScript:
        @"(function() {"
        @"  var ctors = { Error: Error, TypeError: TypeError,"
        @"                RangeError: RangeError, SyntaxError: SyntaxError,"
        @"                ReferenceError: ReferenceError,"
        @"                EvalError: EvalError, URIError: URIError };"
        @"  return function(ctorName, name, message, stack) {"
        @"    var Ctor = ctors[ctorName] || ctors.Error;"
        @"    var error = new Ctor(message);"
        @"    if (name) { error.name = name; }"
        @"    if (stack) { error.stack = stack; }"
        @"    return error;"
        @"  };"
        @"})()"];
    promiseBridge_ = (__bridge_retained void *)[ctx evaluateScript:
        @"(function() {"
        @"  var P = Promise;"
        @"  return function() {"
        @"    var resolve, reject;"
        @"    var promise = new P(function(res, rej) {"
        @"      resolve = res; reject = rej;"
        @"    });"
        @"    return [promise, resolve, reject];"
        @"  };"
        @"})()"];
    if (ctx.exception || !errorFactory_ || !promiseBridge_) {
      NSString *reason = ctx.exception ? safeExceptionMessage(ctx.exception)
                                       : @"intrinsic capture failed";
      ctx.exception = nil;
      // This constructor is aborting, so dispose() will not release them.
      if (errorFactory_) {
        (void)(__bridge_transfer JSValue *)errorFactory_;
        errorFactory_ = nullptr;
      }
      if (promiseBridge_) {
        (void)(__bridge_transfer JSValue *)promiseBridge_;
        promiseBridge_ = nullptr;
      }
      throw jsi::JSError(hostRuntime, [reason UTF8String]);
    }

    // WIP, dev-only: opt this sandbox JSContext into Apple's out-of-process
    // Remote Inspector so a developer can attach Safari's Web Inspector to a
    // running JS guest (JSGlobalContextSetInspectable is public SDK API,
    // macOS 13.3 / iOS 16.4). Quadruple-gated so it can NEVER reach an App
    // Store archive: (1) KEEL_WIP_JSC_INSPECTOR compile flag (off by default),
    // (2) !NDEBUG belt-and-suspenders — a release archive strips this branch
    // even if the flag leaks in, (3) _JSC_HAS_INSPECTABLE SDK guard,
    // (4) __builtin_available runtime OS guard. An inspectable, breakpoint-
    // pausable context is a data-exposure / tenant-isolation hole, so the
    // NDEBUG gate is load-bearing. Not CDP/Chrome — Safari Web Inspector only.
    // See docs/architecture/cdp-debugging.zh.md.
#if defined(KEEL_WIP_JSC_INSPECTOR) && !defined(NDEBUG)
#if !defined(_JSC_HAS_INSPECTABLE)
#if (defined(__IPHONE_OS_VERSION_MAX_ALLOWED) &&                                \
     __IPHONE_OS_VERSION_MAX_ALLOWED >= 160400) ||                             \
    (defined(__MAC_OS_X_VERSION_MAX_ALLOWED) &&                                \
     __MAC_OS_X_VERSION_MAX_ALLOWED >= 130300)
#define _JSC_HAS_INSPECTABLE
#endif
#endif
#ifdef _JSC_HAS_INSPECTABLE
    if (__builtin_available(macOS 13.3, iOS 16.4, *)) {
      ctx.name = @"keel guest (JSC, dev inspector)";
      JSGlobalContextSetInspectable(ctx.JSGlobalContextRef, true);
      NSLog(@"[JSCSandbox] Remote Inspector ENABLED (dev-only) — attach via "
            @"Safari > Develop");
    }
#endif
#endif

    jsContext_ = (__bridge_retained void *)ctx;
  }
}

JSCSandboxContext::~JSCSandboxContext() { dispose(); }

void JSCSandboxContext::dispose() {
  std::lock_guard<std::recursive_mutex> lock(mutex_);
  if (disposed_)
    return;
  disposed_ = true;

  if (jsContext_) {
    @autoreleasepool {
      // Drop outstanding guest resolving functions so their strong references
      // never outlive the JSContext.
      for (auto &weakBridge : promiseBridges_) {
        if (auto bridge = weakBridge.lock()) {
          bridge->settled = true;
          bridge->resolve = nil;
          bridge->reject = nil;
        }
      }
      promiseBridges_.clear();
      // Release the retained realm intrinsics before the context itself.
      if (errorFactory_) {
        (void)(__bridge_transfer JSValue *)errorFactory_;
        errorFactory_ = nullptr;
      }
      if (promiseBridge_) {
        (void)(__bridge_transfer JSValue *)promiseBridge_;
        promiseBridge_ = nullptr;
      }
      // Transfer ownership to ARC and let it release
      (void)(__bridge_transfer JSContext *)jsContext_;
    }
    jsContext_ = nullptr;
  }
  // The group was owned by the released context's VM; never touch it again.
  contextGroup_ = nullptr;
  timeLimitEnabled_ = false;
  callbacks_.clear();
}

// MARK: - TimeLimitScope

JSCSandboxContext::TimeLimitScope::TimeLimitScope(JSCSandboxContext &ctx)
    : ctx_(ctx) {
  // Guard the budget before handing it to JSC: NaN and <= 0 are rejected by
  // the `> 0` comparison, and non-finite / absurdly large values (Infinity,
  // Number.MAX_VALUE, ...) are treated as "no limit" rather than armed.
  // Only the outermost entry arms, so nested host<->sandbox bounces keep the
  // original deadline.
  constexpr double kMaxTimeoutMs = 9.0e15; // ~285k years; clearly "unlimited"
  if (ctx_.timeLimitEnabled_ && ctx_.timeoutMs_ > 0 &&
      ctx_.timeoutMs_ < kMaxTimeoutMs && ctx_.timeLimitDepth_ == 0) {
    ctx_.timeLimitFired_.store(false, std::memory_order_relaxed);
    // The private API takes the limit in SECONDS; our option is milliseconds.
    timeLimitAPI().set((JSContextGroupRef)ctx_.contextGroup_,
                       ctx_.timeoutMs_ / 1000.0, &timeLimitShouldTerminate,
                       &ctx_.timeLimitFired_);
    armedHere_ = true;
  }
  ctx_.timeLimitDepth_++;
}

JSCSandboxContext::TimeLimitScope::~TimeLimitScope() {
  ctx_.timeLimitDepth_--;
  // contextGroup_ can only go null via dispose(); guard against a dispose()
  // issued from a host callback while this scope was still armed.
  if (armedHere_ && ctx_.contextGroup_) {
    timeLimitAPI().clear((JSContextGroupRef)ctx_.contextGroup_);
  }
}

bool JSCSandboxContext::TimeLimitScope::timedOut() const {
  return armedHere_ && ctx_.timeLimitFired_.load(std::memory_order_acquire);
}

jsi::Value JSCSandboxContext::get(jsi::Runtime &rt,
                                  const jsi::PropNameID &name) {
  std::string propName = name.utf8(rt);

  if (propName == "eval") {
    return jsi::Function::createFromHostFunction(
        rt, name,
        1, // argc
        [this](jsi::Runtime &rt, const jsi::Value &thisVal,
               const jsi::Value *args, size_t count) -> jsi::Value {
          (void)thisVal;
          if (count < 1 || !args[0].isString()) {
            throw jsi::JSError(rt, "eval requires a string argument");
          }
          std::string code = args[0].asString(rt).utf8(rt);
          return this->eval(rt, code);
        });
  }

  if (propName == "inject") {
    return jsi::Function::createFromHostFunction(
        rt, name,
        2, // argc
        [this](jsi::Runtime &rt, const jsi::Value &thisVal,
               const jsi::Value *args, size_t count) -> jsi::Value {
          (void)thisVal;
          if (count < 2 || !args[0].isString()) {
            throw jsi::JSError(rt,
                               "inject requires (name: string, value: any)");
          }
          std::string globalName = args[0].asString(rt).utf8(rt);
          this->inject(rt, globalName, args[1]);
          return jsi::Value::undefined();
        });
  }

  if (propName == "extract") {
    return jsi::Function::createFromHostFunction(
        rt, name,
        1, // argc
        [this](jsi::Runtime &rt, const jsi::Value &thisVal,
               const jsi::Value *args, size_t count) -> jsi::Value {
          (void)thisVal;
          if (count < 1 || !args[0].isString()) {
            throw jsi::JSError(rt, "extract requires a string argument");
          }
          std::string globalName = args[0].asString(rt).utf8(rt);
          return this->extract(rt, globalName);
        });
  }

  if (propName == "dispose") {
    return jsi::Function::createFromHostFunction(
        rt, name, 0,
        [this](jsi::Runtime &rt, const jsi::Value &thisVal,
               const jsi::Value *args, size_t count) -> jsi::Value {
          (void)rt;
          (void)thisVal;
          (void)args;
          (void)count;
          this->dispose();
          return jsi::Value::undefined();
        });
  }

  if (propName == "isDisposed") {
    return jsi::Value(disposed_);
  }

  return jsi::Value::undefined();
}

void JSCSandboxContext::set(jsi::Runtime &rt, const jsi::PropNameID &name,
                            const jsi::Value &value) {
  (void)rt;
  (void)name;
  (void)value;
  // Read-only
}

std::vector<jsi::PropNameID>
JSCSandboxContext::getPropertyNames(jsi::Runtime &rt) {
  std::vector<jsi::PropNameID> props;
  props.push_back(jsi::PropNameID::forUtf8(rt, "eval"));
  props.push_back(jsi::PropNameID::forUtf8(rt, "inject"));
  props.push_back(jsi::PropNameID::forUtf8(rt, "extract"));
  props.push_back(jsi::PropNameID::forUtf8(rt, "dispose"));
  props.push_back(jsi::PropNameID::forUtf8(rt, "isDisposed"));
  return props;
}

jsi::Value JSCSandboxContext::eval(jsi::Runtime &rt, const std::string &code) {
  std::lock_guard<std::recursive_mutex> lock(mutex_);

  if (disposed_) {
    throw jsi::JSError(rt, "Context has been disposed");
  }

  @autoreleasepool {
    JSContext *ctx = (__bridge JSContext *)jsContext_;
    NSString *nsCode = [NSString stringWithUTF8String:code.c_str()];

    // Arm the wall-clock execution deadline for this top-level eval. No-op
    // unless enableExecutionTimeLimit was requested at createRuntime and the
    // private JSC API resolved (timeLimitEnabled_), and timeoutMs_ > 0.
    TimeLimitScope timeLimit(*this);
    JSValue *result = [ctx evaluateScript:nsCode];

    // Translate JSC's opaque termination exception into a clear timeout
    // error. The context itself stays valid and usable for later evals.
    if (timeLimit.timedOut()) {
      ctx.exception = nil;
      throw jsi::JSError(
          rt, "JSC eval timed out after " +
                  std::to_string(static_cast<long long>(timeoutMs_)) +
                  "ms (execution interrupted)");
    }

    // Check for exceptions
    if (ctx.exception) {
      NSString *errorMsg = safeExceptionMessage(ctx.exception);
      ctx.exception = nil;
      throw jsi::JSError(rt, [errorMsg UTF8String]);
    }

    return jsValueToJSI(rt, (__bridge void *)result);
  }
}

void JSCSandboxContext::inject(jsi::Runtime &rt, const std::string &name,
                                  const jsi::Value &value) {
  std::lock_guard<std::recursive_mutex> lock(mutex_);

  if (disposed_) {
    throw jsi::JSError(rt, "Context has been disposed");
  }

  @autoreleasepool {
    JSContext *ctx = (__bridge JSContext *)jsContext_;
    NSString *nsName = [NSString stringWithUTF8String:name.c_str()];

    // Handle functions specially - create a wrapper function in JS that calls
    // our block This ensures typeof returns "function" instead of "object"
    if (value.isObject() && value.asObject(rt).isFunction(rt)) {
      jsi::Function func = value.asObject(rt).asFunction(rt);
      
      // DEBUG: Log when registering a function global (especially __sendToHost)
      NSLog(@"[JSCSandbox] inject: registering function '%s', rt=%p, hostRuntime_=%p, same=%d", 
            name.c_str(), (void*)&rt, (void*)hostRuntime_, (&rt == hostRuntime_));
      
      void* jsValPtr = wrapFunctionForSandbox(rt, std::move(func));
      JSValue* jsVal = (__bridge JSValue*)jsValPtr;

      ctx[nsName] = jsVal;
    } else {
      // Convert and set non-function values
      void *jsValue = jsiToJSValue(rt, value);
      JSValue *jsVal = (__bridge JSValue *)jsValue;

      // Use a temp name to store value, then assign to both ctx and globalThis
      static int nonFuncCounter = 0;
      NSString *tempName =
          [NSString stringWithFormat:@"__jsc_tmp_%d__", ++nonFuncCounter];
      ctx[tempName] = jsVal;
      ctx[nsName] = jsVal;

      // Set on globalThis using temp name which is accessible
      NSString *globalThisScript = [NSString
          stringWithFormat:@"(function() { globalThis['%@'] = %@; })()", nsName,
                           tempName];
      [ctx evaluateScript:globalThisScript];
    }
  }
}

jsi::Value JSCSandboxContext::extract(jsi::Runtime &rt,
                                        const std::string &name) {
  std::lock_guard<std::recursive_mutex> lock(mutex_);

  if (disposed_) {
    throw jsi::JSError(rt, "Context has been disposed");
  }

  @autoreleasepool {
    JSContext *ctx = (__bridge JSContext *)jsContext_;
    NSString *nsName = [NSString stringWithUTF8String:name.c_str()];
    JSValue *value = ctx[nsName];
    return jsValueToJSI(rt, (__bridge void *)value);
  }
}

// Helper: wrap a jsi::Function into a JSValue function callable in the sandbox
void *JSCSandboxContext::wrapFunctionForSandbox(jsi::Runtime &rt,
                                                jsi::Function &&func) {
  JSContext *ctx = (__bridge JSContext *)jsContext_;
  jsi::Object funcObj = std::move(func); // Treat as object to access properties

  // 1. Try to retrieve existing Proxy ID from the function object
  std::string internalNameStr;
  bool isCached = false;
  
  try {
    if (funcObj.hasProperty(rt, "__keel_proxy_id__")) {
      jsi::Value idVal = funcObj.getProperty(rt, "__keel_proxy_id__");
      if (idVal.isString()) {
        std::string id = idVal.asString(rt).utf8(rt);
        internalNameStr = "__jsc_fn_" + id + "__";
        
        // Check if this ID actually exists in our native registry
        // (It might be from a different runtime if we are unlucky, but strict isolation should prevent that.
        //  However, to be safe, we rely on our map lookup).
        if (callbacks_.find(internalNameStr) != callbacks_.end()) {
             isCached = true;
        }
      }
    }
  } catch (...) {
    // Ignore errors reading property (e.g. frozen)
  }

      if (isCached) {
        // Retrieve existing wrapper
        
        // Let's extract ID from internalNameStr or just assume format.
        // internalNameStr = "__jsc_fn_123__"
        // replace "fn" with "wrap" -> "__jsc_wrap_123__"
        
        std::string wrapNameStr = internalNameStr;
        size_t fnPos = wrapNameStr.find("_fn_");
        if (fnPos != std::string::npos) {
             wrapNameStr.replace(fnPos, 4, "_wrap_");
        }
        
        NSString *wrapperName = [NSString stringWithUTF8String:wrapNameStr.c_str()];
        JSValue *wrapperFn = ctx[wrapperName];
        
        // If wrapper is missing for some reason (e.g. somehow collected or not set?), fallback to recreate
        if (wrapperFn && ![wrapperFn isUndefined]) {
             return (__bridge void *)wrapperFn;
        }
        // Fallthrough to recreate if missing (shouldn't happen with our logic)
      }
      
      // 3. Not cached: Generate new ID and store
      static int funcCounter = 0;
      int funcId = ++funcCounter;
      std::string idStr = std::to_string(funcId);
      NSString *internalName = [NSString stringWithFormat:@"__jsc_fn_%d__", funcId];
      NSString *wrapperName = [NSString stringWithFormat:@"__jsc_wrap_%d__", funcId];
      internalNameStr = [internalName UTF8String];
      
      // DEBUG: Log funcId assignment
      NSLog(@"[JSCSandbox] wrapFunctionForSandbox: assigned funcId=%d", funcId);

      // Tag the original function with the ID
      try {
          funcObj.setProperty(rt, "__keel_proxy_id__", jsi::String::createFromUtf8(rt, idStr));
      } catch (...) {
          // Failed to tag, skip caching
      }
      
      // Convert back to function for storage
      jsi::Function funcToStore = funcObj.asFunction(rt);
      
      // CRITICAL FIX: Instead of storing the jsi::Function directly,
      // create a HostFunction wrapper that captures the original function.
      // This ensures the function is called properly within the Host Runtime context.
      auto sharedFunc = std::make_shared<jsi::Function>(std::move(funcToStore));
      jsi::Runtime* capturedRt = &rt; // Capture the original runtime pointer
      
      // Create a HostFunction that wraps the original JS function
      jsi::Function hostFuncWrapper = jsi::Function::createFromHostFunction(
          rt,
          jsi::PropNameID::forUtf8(rt, internalNameStr),
          0, // We don't know the exact arg count, but it doesn't matter for HostFunction
          [sharedFunc, funcId, capturedRt](jsi::Runtime& callRt, const jsi::Value& /*thisVal*/,
                               const jsi::Value* args, size_t count) -> jsi::Value {
              NSLog(@"[JSCSandbox] fn_%d HostFunction wrapper: callRt=%p, capturedRt=%p, same=%d", 
                    funcId, (void*)&callRt, (void*)capturedRt, (&callRt == capturedRt));
              try {
                  // Use the CAPTURED runtime, not the callRt!
                  jsi::Value result = sharedFunc->call(*capturedRt, args, count);
                  NSLog(@"[JSCSandbox] fn_%d HostFunction wrapper: call succeeded", funcId);
                  return result;
              } catch (const std::exception& e) {
                  NSLog(@"[JSCSandbox] fn_%d HostFunction wrapper: exception: %s", funcId, e.what());
                  throw;
              }
          }
      );

      // Store the HostFunction wrapper in callbacks_
      callbacks_[internalNameStr] =
          std::make_shared<jsi::Function>(std::move(hostFuncWrapper));

      // Get reference to host runtime
      jsi::Runtime *hostRt = hostRuntime_;
      auto self = this;

      // Create native block that calls the stored function
      JSValue *blockFn = [JSValue
          valueWithObject:^id(void) {
            NSArray *args = [JSContext currentArguments];

            @autoreleasepool {
              std::lock_guard<std::recursive_mutex> lock(self->mutex_);

              auto it = self->callbacks_.find(internalNameStr);
              if (it == self->callbacks_.end()) {
                NSLog(@"[JSCSandbox] Wrapped fn_%d: function not found!", funcId);
                return nil;
              }

          // DEBUG: Log incoming arguments for troubleshooting
          if (args.count > 0) {
            JSValue *firstArg = args[0];
            if ([firstArg isObject]) {
              // Check if this looks like an operation batch
              JSValue *operations = firstArg[@"operations"];
              if (operations && [operations isArray]) {
                NSUInteger opCount = [[operations[@"length"] toNumber] unsignedIntegerValue];
                NSLog(@"[JSCSandbox] fn_%d called with batch of %lu operations", funcId, (unsigned long)opCount);
                
                // Log each operation's props keys
                for (NSUInteger i = 0; i < opCount && i < 5; i++) {
                  JSValue *op = [operations valueAtIndex:i];
                  JSValue *opType = op[@"type"];
                  JSValue *props = op[@"props"];
                  if (props && [props isObject]) {
                    // Get keys of props using JSON.stringify for better visibility
                    JSContext *ctx = (__bridge JSContext *)self->jsContext_;
                    JSValue *stringifyFunc = [ctx evaluateScript:@"(function(o) { try { return JSON.stringify(o); } catch(e) { return '(error: ' + e.message + ')'; } })"];
                    JSValue *jsonStr = [stringifyFunc callWithArguments:@[props]];
                    NSLog(@"[JSCSandbox] fn_%d op[%lu] type=%@ props=%@", 
                          funcId, (unsigned long)i, 
                          [opType isString] ? [opType toString] : @"(non-string)",
                          [jsonStr toString]);
                    
                    // Also check if props has onPress specifically
                    JSValue *onPress = props[@"onPress"];
                    if (![onPress isUndefined]) {
                      NSLog(@"[JSCSandbox] fn_%d op[%lu] has onPress: isObject=%d, isString=%d",
                            funcId, (unsigned long)i, [onPress isObject], [onPress isString]);
                      if ([onPress isObject]) {
                        JSValue *typeField = onPress[@"__type"];
                        JSValue *fnIdField = onPress[@"__fnId"];
                        NSLog(@"[JSCSandbox] fn_%d op[%lu] onPress.__type=%@, __fnId=%@",
                              funcId, (unsigned long)i,
                              [typeField isString] ? [typeField toString] : @"(undefined)",
                              [fnIdField isString] ? [fnIdField toString] : @"(undefined)");
                      }
                    }
                  }
                }
              }
            }
          }

          try {
            // Convert JSValue args to jsi::Value
            std::vector<jsi::Value> jsiArgs;
            for (NSUInteger i = 0; i < args.count; i++) {
              JSValue *arg = args[i];
              jsiArgs.push_back(
                  self->jsValueToJSI(*hostRt, (__bridge void *)arg));
            }
            
            // DEBUG: After conversion, verify the first arg (batch) structure
            if (jsiArgs.size() > 0 && jsiArgs[0].isObject()) {
              jsi::Object batchObj = jsiArgs[0].asObject(*hostRt);
              if (batchObj.hasProperty(*hostRt, "operations")) {
                jsi::Value opsVal = batchObj.getProperty(*hostRt, "operations");
                if (opsVal.isObject() && opsVal.asObject(*hostRt).isArray(*hostRt)) {
                  jsi::Array opsArr = opsVal.asObject(*hostRt).asArray(*hostRt);
                  size_t opCount = opsArr.size(*hostRt);
                  NSLog(@"[JSCSandbox] fn_%d: jsiArgs batch has %zu operations", funcId, opCount);
                  
                  // Check each operation for TouchableOpacity props
                  for (size_t i = 0; i < opCount && i < 5; i++) {
                    jsi::Value opVal = opsArr.getValueAtIndex(*hostRt, i);
                    if (opVal.isObject()) {
                      jsi::Object opObj = opVal.asObject(*hostRt);
                      if (opObj.hasProperty(*hostRt, "type")) {
                        jsi::Value typeVal = opObj.getProperty(*hostRt, "type");
                        if (typeVal.isString()) {
                          std::string typeStr = typeVal.asString(*hostRt).utf8(*hostRt);
                          if (typeStr == "TouchableOpacity" && opObj.hasProperty(*hostRt, "props")) {
                            jsi::Value propsVal = opObj.getProperty(*hostRt, "props");
                            if (propsVal.isObject()) {
                              jsi::Object propsObj = propsVal.asObject(*hostRt);
                              jsi::Array propNames = propsObj.getPropertyNames(*hostRt);
                              size_t propCount = propNames.size(*hostRt);
                              std::string propList;
                              for (size_t j = 0; j < propCount && j < 10; j++) {
                                if (j > 0) propList += ", ";
                                propList += propNames.getValueAtIndex(*hostRt, j).asString(*hostRt).utf8(*hostRt);
                              }
                              NSLog(@"[JSCSandbox] fn_%d: jsiArgs op[%zu] TouchableOpacity props (%zu): %s", 
                                    funcId, i, propCount, propList.c_str());
                              
                              // Check specifically for onPress
                              if (propsObj.hasProperty(*hostRt, "onPress")) {
                                jsi::Value onPressVal = propsObj.getProperty(*hostRt, "onPress");
                                NSLog(@"[JSCSandbox] fn_%d: jsiArgs op[%zu] onPress isUndefined=%d isObject=%d",
                                      funcId, i, onPressVal.isUndefined(), onPressVal.isObject());
                              } else {
                                NSLog(@"[JSCSandbox] fn_%d: jsiArgs op[%zu] NO onPress property!!", funcId, i);
                              }
                            }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }

            jsi::Value result;
            
            // DEBUG: Log before calling Host function
            NSLog(@"[JSCSandbox] fn_%d: ABOUT TO CALL Host function with %zu args", funcId, jsiArgs.size());
            NSLog(@"[JSCSandbox] fn_%d: callbacks_.size=%lu, internalNameStr=%s", 
                  funcId, (unsigned long)self->callbacks_.size(), internalNameStr.c_str());
            
            // DEBUG: Verify the stored function is valid
            jsi::Function& storedFunc = *it->second;
            bool isHostObject = storedFunc.isHostObject(*hostRt);
            bool isHostFunc = storedFunc.isHostFunction(*hostRt);
            NSLog(@"[JSCSandbox] fn_%d: storedFunc isHostObject=%d, isHostFunction=%d", 
                  funcId, isHostObject, isHostFunc);
            
            // DEBUG: Final verification of jsiArgs content just before calling Host function
            // This verifies that jsi::Value objects are intact at the point of the call
            if (jsiArgs.size() > 0 && jsiArgs[0].isObject()) {
              jsi::Object arg0 = jsiArgs[0].asObject(*hostRt);
              if (arg0.hasProperty(*hostRt, "operations")) {
                jsi::Value opsVal = arg0.getProperty(*hostRt, "operations");
                if (opsVal.isObject() && opsVal.asObject(*hostRt).isArray(*hostRt)) {
                  jsi::Array opsArr = opsVal.asObject(*hostRt).asArray(*hostRt);
                  size_t opCount = opsArr.size(*hostRt);
                  NSLog(@"[JSCSandbox] fn_%d: FINAL CHECK - batch has %zu operations", funcId, opCount);
                  
                  for (size_t i = 0; i < opCount && i < 5; i++) {
                    jsi::Value opVal = opsArr.getValueAtIndex(*hostRt, i);
                    if (opVal.isObject()) {
                      jsi::Object opObj = opVal.asObject(*hostRt);
                      if (opObj.hasProperty(*hostRt, "type")) {
                        jsi::Value typeVal = opObj.getProperty(*hostRt, "type");
                        if (typeVal.isString()) {
                          std::string typeStr = typeVal.asString(*hostRt).utf8(*hostRt);
                          if (typeStr == "TouchableOpacity" && opObj.hasProperty(*hostRt, "props")) {
                            jsi::Value propsVal = opObj.getProperty(*hostRt, "props");
                            if (propsVal.isObject()) {
                              jsi::Object propsObj = propsVal.asObject(*hostRt);
                              
                              // Check if onPress exists
                              if (propsObj.hasProperty(*hostRt, "onPress")) {
                                jsi::Value onPressVal = propsObj.getProperty(*hostRt, "onPress");
                                NSLog(@"[JSCSandbox] fn_%d: FINAL CHECK op[%zu] onPress exists! isObject=%d isFunction=%d isUndefined=%d",
                                      funcId, i, onPressVal.isObject(), 
                                      onPressVal.isObject() && onPressVal.asObject(*hostRt).isFunction(*hostRt),
                                      onPressVal.isUndefined());
                                
                                // If it's an object, check its properties
                                if (onPressVal.isObject()) {
                                  jsi::Object onPressObj = onPressVal.asObject(*hostRt);
                                  jsi::Array onPressPropNames = onPressObj.getPropertyNames(*hostRt);
                                  size_t onPressPropCount = onPressPropNames.size(*hostRt);
                                  std::string propList;
                                  for (size_t j = 0; j < onPressPropCount && j < 10; j++) {
                                    if (j > 0) propList += ", ";
                                    propList += onPressPropNames.getValueAtIndex(*hostRt, j).asString(*hostRt).utf8(*hostRt);
                                  }
                                  NSLog(@"[JSCSandbox] fn_%d: FINAL CHECK op[%zu] onPress object has %zu props: %s",
                                        funcId, i, onPressPropCount, propList.c_str());
                                  
                                  // Check __type
                                  if (onPressObj.hasProperty(*hostRt, "__type")) {
                                    jsi::Value typeVal = onPressObj.getProperty(*hostRt, "__type");
                                    if (typeVal.isString()) {
                                      NSLog(@"[JSCSandbox] fn_%d: FINAL CHECK op[%zu] onPress.__type = %s",
                                            funcId, i, typeVal.asString(*hostRt).utf8(*hostRt).c_str());
                                    }
                                  }
                                }
                              } else {
                                // List all props
                                jsi::Array propNames = propsObj.getPropertyNames(*hostRt);
                                size_t propCount = propNames.size(*hostRt);
                                std::string propList;
                                for (size_t j = 0; j < propCount && j < 10; j++) {
                                  if (j > 0) propList += ", ";
                                  propList += propNames.getValueAtIndex(*hostRt, j).asString(*hostRt).utf8(*hostRt);
                                }
                                NSLog(@"[JSCSandbox] fn_%d: FINAL CHECK op[%zu] NO onPress! props are: %s",
                                      funcId, i, propList.c_str());
                              }
                            }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }
            
            if (jsiArgs.empty()) {
              NSLog(@"[JSCSandbox] fn_%d: calling Host function with NO args", funcId);
              result = it->second->call(*hostRt);
            } else {
              NSLog(@"[JSCSandbox] fn_%d: calling Host function with %zu args, hostRt=%p", funcId, jsiArgs.size(), (void*)hostRt);
              
              // DEBUG: Try JSON.stringify on the first arg to see what Host Runtime sees
              try {
                jsi::Function stringify = hostRt->global()
                    .getPropertyAsObject(*hostRt, "JSON")
                    .getPropertyAsFunction(*hostRt, "stringify");
                jsi::Value jsonStr = stringify.call(*hostRt, jsiArgs[0]);
                if (jsonStr.isString()) {
                  std::string str = jsonStr.asString(*hostRt).utf8(*hostRt);
                  // Truncate to avoid log spam
                  if (str.length() > 500) {
                    str = str.substr(0, 500) + "...";
                  }
                  NSLog(@"[JSCSandbox] fn_%d: JSON.stringify(arg[0])=%s", funcId, str.c_str());
                }
              } catch (const std::exception& e) {
                NSLog(@"[JSCSandbox] fn_%d: JSON.stringify failed: %s", funcId, e.what());
              }
              
              result = it->second->call(
                  *hostRt, (const jsi::Value *)jsiArgs.data(), jsiArgs.size());
            }
            
            // DEBUG: Log after calling Host function
            NSLog(@"[JSCSandbox] fn_%d: Host function RETURNED, result isUndefined=%d isObject=%d isString=%d",
                  funcId, result.isUndefined(), result.isObject(), result.isString());

            void *jsResult = self->jsiToJSValue(*hostRt, result);
            NSLog(@"[JSCSandbox] fn_%d: jsiToJSValue completed, returning to Guest", funcId);
            return (__bridge id)jsResult;
          } catch (const std::exception &e) {
            os_log_error(OS_LOG_DEFAULT, "[JSCSandbox] fn_%d exception: %{public}s", funcId, e.what());
            return nil;
          } catch (...) {
            os_log_error(OS_LOG_DEFAULT, "[JSCSandbox] fn_%d unknown exception!", funcId);
            return nil;
          }
        }
      }
            inContext:ctx];

  // Store the block function with internal name
  ctx[internalName] = blockFn;

  // Create a proper function wrapper using eval (ensures typeof returns
  // "function")
  NSString *wrapperScript = [NSString
      stringWithFormat:@"(function() { var fn = %@; var w = function(...args) { "
                       @"return fn(...args); }; return w; })()",
                       internalName];
  JSValue *wrapperFn = [ctx evaluateScript:wrapperScript];

  // Store it in the context so we can retrieve it later for caching
  ctx[wrapperName] = wrapperFn;

  return (__bridge void *)wrapperFn;
}

// Host errors must become real sandbox Errors. Rebuilding them in the guest
// realm keeps `instanceof Error`/`instanceof TypeError` and the standard
// name/message/stack fields working for host capability rejections; the
// generic own-property copy would hand the guest a plain object instead.
// Returns nullptr when `obj` is not a host Error.
void *JSCSandboxContext::tryMakeSandboxError(jsi::Runtime &rt,
                                             const jsi::Object &obj,
                                             ConversionState &state) {
  std::string ctorName = "Error";
  try {
    // Cheap own-property probe before the prototype walk: most plain data
    // objects never carry `message`.
    if (!obj.hasProperty(rt, "message")) {
      return nullptr;
    }
    // Detect the Error brand through the HOST realm's own prototype chain.
    // `instanceof` is realm-local here (both operands are host values), so it
    // cannot be spoofed the way a Symbol.toStringTag tag check can.
    jsi::Function errorCtor = rt.global().getPropertyAsFunction(rt, "Error");
    if (!obj.instanceOf(rt, errorCtor)) {
      return nullptr;
    }
    static const char *kErrorSubtypes[] = {"TypeError",   "RangeError",
                                           "SyntaxError", "ReferenceError",
                                           "EvalError",   "URIError"};
    for (const char *subtype : kErrorSubtypes) {
      jsi::Function ctor = rt.global().getPropertyAsFunction(rt, subtype);
      if (obj.instanceOf(rt, ctor)) {
        ctorName = subtype;
        break;
      }
    }
  } catch (const jsi::JSError &) {
    return nullptr;
  }

  // A cycle back to an Error already being converted reuses the guest Error.
  if (void *existing = lookupConvertedObject(rt, obj, state)) {
    return existing;
  }

  // A throwing getter on one field must not drop the Error brand; keep the
  // fields that are readable.
  auto readString = [&](const char *key) -> std::string {
    try {
      jsi::Value val = obj.getProperty(rt, key);
      if (val.isString()) {
        return val.asString(rt).utf8(rt);
      }
    } catch (const jsi::JSError &) {
      // Ignore this field only.
    }
    return std::string();
  };
  std::string name = readString("name");
  std::string message = readString("message");
  std::string stack = readString("stack");

  JSContext *ctx = (__bridge JSContext *)jsContext_;
  NSString *nsCtorName = [NSString stringWithUTF8String:ctorName.c_str()];
  NSString *nsName = [NSString stringWithUTF8String:name.c_str()];
  NSString *nsMessage = [NSString stringWithUTF8String:message.c_str()];
  NSString *nsStack = [NSString stringWithUTF8String:stack.c_str()];
  JSValue *factory = (__bridge JSValue *)errorFactory_;
  JSValue *guestError = factory ? [factory callWithArguments:@[
    nsCtorName ?: @"Error", nsName ?: @"", nsMessage ?: @"", nsStack ?: @""
  ]] : nil;
  if (ctx.exception || !guestError || ![guestError isObject]) {
    // Never leave a pending guest exception: it would surface on the next
    // unrelated eval. Fall back to JSC's intrinsic Error constructor (the C
    // API does not consult the tenant-mutable globalThis.Error).
    if (ctx.exception) {
      ctx.exception = nil;
    }
    guestError = [JSValue valueWithNewErrorFromMessage:(nsMessage ?: @"")
                                             inContext:ctx];
  }

  // Register the rebuilt Error for the duration of its custom-field subtree
  // so cycles resolve to the same guest object instead of recursing.
  ConversionScope conversionScope(state, rt, obj,
                                   (__bridge void *)guestError);

  // Preserve custom own enumerable fields (e.g. `code`) that the generic
  // converter used to copy. Standard fields always win, and `__proto__` is
  // skipped so a data field cannot rewrite the guest prototype.
  if ([guestError isObject]) {
    try {
      jsi::Function objectKeys =
          rt.global()
              .getPropertyAsObject(rt, "Object")
              .getPropertyAsFunction(rt, "keys");
      jsi::Value keysValue = objectKeys.call(rt, obj);
      jsi::Array keys = keysValue.asObject(rt).asArray(rt);
      size_t keyCount = keys.size(rt);
      for (size_t i = 0; i < keyCount; i++) {
        jsi::Value keyValue = keys.getValueAtIndex(rt, i);
        if (!keyValue.isString()) {
          continue;
        }
        std::string key = keyValue.asString(rt).utf8(rt);
        if (key == "name" || key == "message" || key == "stack" ||
            key == "__proto__") {
          continue;
        }
        NSString *nsKey = [NSString stringWithUTF8String:key.c_str()];
        if (!nsKey) {
          continue;
        }
        try {
          jsi::Value fieldValue = obj.getProperty(rt, key.c_str());
          void *jsField = jsiToJSValueImpl(rt, fieldValue, true, state);
          if (jsField) {
            guestError[nsKey] = (__bridge JSValue *)jsField;
          }
        } catch (const jsi::JSError &) {
          // Skip only the unreadable field.
        }
      }
    } catch (const jsi::JSError &) {
      // Custom fields are best-effort; the rebuilt Error remains valid.
    }
  }
  return (__bridge void *)guestError;
}

// Convert the value thrown by the host into a sandbox rejection reason.
// Promise semantics reject with the thrown value as-is: a standard Error keeps
// its subtype through tryMakeSandboxError, primitives stay primitives, and a
// thenable is NOT adopted (rejection reasons are never unwrapped).
void *JSCSandboxContext::sandboxRejectionReason(jsi::Runtime &rt,
                                                const jsi::JSError &error) {
  try {
    return jsiToJSValue(rt, error.value(), /*unwrapThenables=*/false);
  } catch (const jsi::JSError &) {
    // Host getters on the thrown value can throw too; still hand the guest a
    // usable reason instead of letting the conversion failure escape.
    JSContext *ctx = (__bridge JSContext *)jsContext_;
    NSString *nsMessage =
        [NSString stringWithUTF8String:error.getMessage().c_str()];
    return (__bridge void *)[JSValue valueWithNewErrorFromMessage:
        (nsMessage ?: @"") inContext:ctx];
  }
}

// Settle a sandbox promise via its reject callback. Rejecting an
// already-settled promise is a no-op, so a thenable that settles and then
// throws still follows Promise semantics (the first settlement wins).
void JSCSandboxContext::rejectSandboxPromise(void *rejectFn, void *reason) {
  JSContext *ctx = (__bridge JSContext *)jsContext_;
  JSValue *reject = (__bridge JSValue *)rejectFn;
  if (!reject) {
    return;
  }
  JSValue *nsReason = reason ? (__bridge JSValue *)reason
                             : [JSValue valueWithUndefinedInContext:ctx];
  [reject callWithArguments:@[nsReason]];
  if (ctx.exception) {
    ctx.exception = nil;
  }
}

// Convert jsi::Value to JSValue* (top-level entry: owns the cycle state)
void *JSCSandboxContext::jsiToJSValue(jsi::Runtime &rt,
                                      const jsi::Value &value,
                                      bool unwrapThenables) {
  ConversionState state;
  return jsiToJSValueImpl(rt, value, unwrapThenables, state);
}

// Convert jsi::Value to JSValue* (recursive: shares the cycle state)
void *JSCSandboxContext::jsiToJSValueImpl(jsi::Runtime &rt,
                                          const jsi::Value &value,
                                          bool unwrapThenables,
                                          ConversionState &state) {
  JSContext *ctx = (__bridge JSContext *)jsContext_;

  if (value.isUndefined()) {
    return (__bridge void *)[JSValue valueWithUndefinedInContext:ctx];
  }
  if (value.isNull()) {
    return (__bridge void *)[JSValue valueWithNullInContext:ctx];
  }
  if (value.isBool()) {
    return (__bridge void *)[JSValue valueWithBool:value.getBool()
                                         inContext:ctx];
  }
  if (value.isNumber()) {
    return (__bridge void *)[JSValue valueWithDouble:value.getNumber()
                                           inContext:ctx];
  }
  if (value.isString()) {
    std::string str = value.asString(rt).utf8(rt);
    NSString *nsStr = [NSString stringWithUTF8String:str.c_str()];
    return (__bridge void *)[JSValue valueWithObject:nsStr inContext:ctx];
  }
  // Handle Symbols
  if (value.isSymbol()) {
    jsi::Symbol sym = value.getSymbol(rt);
    std::string symDesc = sym.toString(rt);
    NSString *nsSymDesc = [NSString stringWithUTF8String:symDesc.c_str()];

    // Try to extract the key from "Symbol(key)" format
    NSRegularExpression *regex =
        [NSRegularExpression regularExpressionWithPattern:@"Symbol\\((.+)\\)"
                                                  options:0
                                                    error:nil];
    NSTextCheckingResult *match =
        [regex firstMatchInString:nsSymDesc
                          options:0
                            range:NSMakeRange(0, nsSymDesc.length)];

    if (match && match.numberOfRanges > 1) {
      NSString *symKey = [nsSymDesc substringWithRange:[match rangeAtIndex:1]];
      // Use Symbol.for() to create a global Symbol with the same key in JSC
      NSString *script =
          [NSString stringWithFormat:@"Symbol.for('%@')", symKey];
      JSValue *jscSymbol = [ctx evaluateScript:script];
      return (__bridge void *)jscSymbol;
    } else {
      // For non-registered Symbols, create a new Symbol with the description
      NSString *script = [NSString stringWithFormat:@"Symbol('%@')", nsSymDesc];
      JSValue *jscSymbol = [ctx evaluateScript:script];
      return (__bridge void *)jscSymbol;
    }
  }
  if (value.isObject()) {
    jsi::Object obj = value.asObject(rt);

    // Handle functions FIRST (before general object handling)
    if (obj.isFunction(rt)) {
      jsi::Function func = obj.asFunction(rt);
      return wrapFunctionForSandbox(rt, std::move(func));
    }

    // Binary passthrough (host -> sandbox): a host capability result carrying
    // an ArrayBuffer / typed-array must reach the guest as real bytes, not the
    // generic Object.keys copy below (which sees zero own props and drops it to
    // {}). Symmetric with jsValueToJSI's sandbox->host branch. Bytes are COPIED
    // into the sandbox realm; the two realms never alias.
    if (obj.isArrayBuffer(rt)) {
      jsi::ArrayBuffer ab = obj.getArrayBuffer(rt);
      return makeSandboxBytes(ctx, ab.data(rt), ab.size(rt), nullptr);
    }
    {
      std::vector<uint8_t> window;
      std::string viewCtor;
      if (extractHostViewBytes(rt, obj, window, viewCtor)) {
        return makeSandboxBytes(ctx, window.empty() ? nullptr : window.data(),
                                window.size(), viewCtor.c_str());
      }
    }

    // Rebuild host Errors as real sandbox Errors before the generic object
    // copy below flattens them into plain objects.
    if (void *sandboxError = tryMakeSandboxError(rt, obj, state)) {
      return sandboxError;
    }

    // Preserve thenables as real sandbox promises. Treating a host Promise
    // as a plain object drops its methods and leaves callers with {}.
    // Rejection reasons are values, never adopted thenables, so the caller can
    // disable this branch with unwrapThenables=false.
    if (unwrapThenables) {
      // Reading `then` (getter) and invoking it can both throw synchronously.
      // Such a failure must settle the new guest promise instead of escaping
      // the conversion: the host-function bridge that called us catches
      // conversion exceptions and returns nil, which would leave the guest
      // with no promise and no way to observe the host error.
      jsi::Value thenValue;
      try {
        thenValue = obj.getProperty(rt, "then");
      } catch (const jsi::JSError &e) {
        JSValue *factory = (__bridge JSValue *)promiseBridge_;
        JSValue *bridge = [factory callWithArguments:@[]];
        rejectSandboxPromise((__bridge void *)bridge[2],
                             sandboxRejectionReason(rt, e));
        return (__bridge void *)bridge[0];
      }
      if (thenValue.isObject() && thenValue.asObject(rt).isFunction(rt)) {
        JSValue *factory = (__bridge JSValue *)promiseBridge_;
        JSValue *bridge = [factory callWithArguments:@[]];
        jsi::Value resolveProxy = jsi::Value::undefined();
        jsi::Value rejectProxy = jsi::Value::undefined();
        auto settlement = makePromiseCallbacks(
            rt, (__bridge void *)bridge[1], (__bridge void *)bridge[2],
            resolveProxy, rejectProxy);
        try {
          thenValue.asObject(rt).asFunction(rt).callWithThis(
              rt, obj, {std::move(resolveProxy), std::move(rejectProxy)});
        } catch (const jsi::JSError &e) {
          // Lock the settlement before converting the thrown reason: that
          // conversion can run host getters which re-enter the proxies. If
          // `then` already settled, beginSettlement returns false and the late
          // throw is ignored (the first settlement wins).
          void *rawResolve = nullptr;
          void *rawReject = nullptr;
          if (beginSettlement(settlement, &rawResolve, &rawReject)) {
            JSValue *resolveFn = (__bridge_transfer JSValue *)rawResolve;
            JSValue *rejectFn = (__bridge_transfer JSValue *)rawReject;
            (void)resolveFn; // A thrown value is always a rejection.
            void *reason = sandboxRejectionReason(rt, e);
            if (rejectFn) {
              invokeSettlement((__bridge void *)rejectFn, reason);
            }
            JSContext *ctx = (__bridge JSContext *)jsContext_;
            if (ctx.exception) {
              ctx.exception = nil;
            }
          }
        }
        return (__bridge void *)bridge[0];
      }
    }

    // A cycle back to an object already being converted reuses the guest
    // value built for it instead of recursing forever.
    if (void *existing = lookupConvertedObject(rt, obj, state)) {
      return existing;
    }

    // Handle arrays
    if (obj.isArray(rt)) {
      jsi::Array arr = obj.asArray(rt);
      size_t len = arr.size(rt);
      // Materialize the guest array before walking elements so a self-cycle
      // resolves to this same guest array.
      JSValue *guestArray = [ctx evaluateScript:@"[]"];
      ConversionScope conversionScope(state, rt, obj,
                                       (__bridge void *)guestArray);
      for (size_t i = 0; i < len; i++) {
        JSValue *elem = (__bridge JSValue *)jsiToJSValueImpl(
            rt, arr.getValueAtIndex(rt, i), true, state);
        guestArray[i] = elem ?: [JSValue valueWithNullInContext:ctx];
      }
      return (__bridge void *)guestArray;
    }

    // Handle plain objects
    jsi::Array propNames = obj.getPropertyNames(rt);
    size_t len = propNames.size(rt);

    // Use JavaScript object directly instead of NSDictionary to preserve
    // functions
    JSValue *jsObj = [ctx evaluateScript:@"({})"];
    ConversionScope conversionScope(state, rt, obj, (__bridge void *)jsObj);

    for (size_t i = 0; i < len; i++) {
      std::string key = propNames.getValueAtIndex(rt, i).asString(rt).utf8(rt);
      NSString *nsKey = [NSString stringWithUTF8String:key.c_str()];
      jsi::Value propVal = obj.getProperty(rt, key.c_str());

      // Convert all values including functions
      JSValue *jsVal = (__bridge JSValue *)jsiToJSValueImpl(
          rt, propVal, true, state);
      if (jsVal) {
        jsObj[nsKey] = jsVal;
      }
    }
    return (__bridge void *)jsObj;
  }

  return (__bridge void *)[JSValue valueWithUndefinedInContext:ctx];
}

// Store a guest function and return a host proxy that invokes it.
jsi::Value JSCSandboxContext::makeFunctionProxy(jsi::Runtime &rt, void *fn) {
  JSContext *ctx = (__bridge JSContext *)jsContext_;
  JSValue *guestFunction = (__bridge JSValue *)fn;

  static int sandboxFuncCounter = 0;
  NSString *funcKey = [NSString
      stringWithFormat:@"__sandbox_fn_%d__", ++sandboxFuncCounter];
  ctx[funcKey] = guestFunction;

  std::string funcKeyStr = [funcKey UTF8String];
  auto *self = this;

  return jsi::Function::createFromHostFunction(
      rt, jsi::PropNameID::forUtf8(rt, "sandboxProxy"),
      0, // variadic
      [self, funcKeyStr](jsi::Runtime &rt, const jsi::Value &thisVal,
                         const jsi::Value *args,
                         size_t count) -> jsi::Value {
        (void)thisVal;
        @autoreleasepool {
          std::lock_guard<std::recursive_mutex> lock(self->mutex_);

          if (self->disposed_) {
            throw jsi::JSError(rt, "Context has been disposed");
          }

          JSContext *ctx = (__bridge JSContext *)self->jsContext_;
          NSString *funcKey = [NSString stringWithUTF8String:funcKeyStr.c_str()];
          JSValue *sandboxFunc = ctx[funcKey];

          if (!sandboxFunc || [sandboxFunc isUndefined]) {
            throw jsi::JSError(rt, "Sandbox function not found");
          }

          // Convert args to JSValue array
          NSMutableArray *jsArgs = [NSMutableArray arrayWithCapacity:count];
          for (size_t i = 0; i < count; i++) {
            JSValue *jsArg =
                (__bridge JSValue *)self->jsiToJSValue(rt, args[i]);
            [jsArgs
                addObject:jsArg
                              ?: [JSValue valueWithUndefinedInContext:ctx]];
          }

          // Host->sandbox function calls execute tenant JS too: same budget
          // as eval (nested entries keep the outermost deadline).
          TimeLimitScope timeLimit(*self);
          JSValue *result = [sandboxFunc callWithArguments:jsArgs];

          if (timeLimit.timedOut()) {
            ctx.exception = nil;
            throw jsi::JSError(
                rt, "JSC sandbox function timed out after " +
                        std::to_string(
                            static_cast<long long>(self->timeoutMs_)) +
                        "ms (execution interrupted)");
          }

          // Check for exceptions
          if (ctx.exception) {
            NSString *errorMsg = safeExceptionMessage(ctx.exception);
            ctx.exception = nil;
            throw jsi::JSError(rt, [errorMsg UTF8String]);
          }

          return self->jsValueToJSI(rt, (__bridge void *)result);
        }
      });
}

bool JSCSandboxContext::invokeSettlement(void *fn, void *arg) {
  JSContext *ctx = (__bridge JSContext *)jsContext_;
  // Calling a guest resolver/rejector can synchronously drain guest Promise
  // reactions, i.e. run tenant code. Keep it inside the same execution budget
  // as eval and host->sandbox calls.
  TimeLimitScope timeLimit(*this);
  [(__bridge JSValue *)fn callWithArguments:@[(__bridge JSValue *)arg]];
  if (timeLimit.timedOut()) {
    // The termination exception must not leak into the next unrelated call.
    ctx.exception = nil;
    return false;
  }
  return true;
}

bool JSCSandboxContext::beginSettlement(
    const std::shared_ptr<PromiseBridge> &bridge, void **resolveOut,
    void **rejectOut) {
  *resolveOut = nullptr;
  *rejectOut = nullptr;
  if (!bridge || bridge->settled) {
    return false;
  }
  // Lock before any conversion: converting the argument can run host getters
  // that re-enter the peer proxy. The FIRST call must win even though its
  // conversion has not finished yet.
  bridge->settled = true;
  JSValue *resolveFn = bridge->resolve;
  JSValue *rejectFn = bridge->reject;
  bridge->resolve = nil;
  bridge->reject = nil;
  if (resolveFn) {
    *resolveOut = (__bridge_retained void *)resolveFn;
  }
  if (rejectFn) {
    // +1: the caller takes ownership with __bridge_transfer and invokes it.
    // A plain __bridge return would leave a dangling pointer once this frame
    // released its strong local.
    *rejectOut = (__bridge_retained void *)rejectFn;
  }
  return true;
}

// One settlement proxy for a guest Promise. A conversion failure must reject
// the guest Promise: the host `.then(...)` return value is discarded, so an
// exception escaping this proxy would leave the guest pending forever.
jsi::Value JSCSandboxContext::makeSettlementProxy(
    jsi::Runtime &rt, const std::shared_ptr<PromiseBridge> &bridge,
    bool isReject) {
  auto *self = this;
  return jsi::Function::createFromHostFunction(
      rt,
      jsi::PropNameID::forUtf8(
          rt, isReject ? "sandboxPromiseReject" : "sandboxPromiseResolve"),
      1, [self, bridge, isReject](jsi::Runtime &rt,
                                  const jsi::Value &thisVal,
                                  const jsi::Value *args,
                                  size_t count) -> jsi::Value {
        (void)thisVal;
        @autoreleasepool {
          std::lock_guard<std::recursive_mutex> lock(self->mutex_);
          if (self->disposed_) {
            return jsi::Value::undefined();
          }

          // Lock the first settlement attempt BEFORE converting the argument:
          // conversion can run host getters that re-enter the peer proxy, and
          // the first call must still win.
          void *rawResolve = nullptr;
          void *rawReject = nullptr;
          if (!self->beginSettlement(bridge, &rawResolve, &rawReject)) {
            return jsi::Value::undefined();
          }
          JSValue *resolveFn = (__bridge_transfer JSValue *)rawResolve;
          JSValue *rejectFn = (__bridge_transfer JSValue *)rawReject;

          JSContext *ctx = (__bridge JSContext *)self->jsContext_;
          void *converted = nullptr;
          std::string conversionError;
          jsi::Value undefined;
          const jsi::Value &arg = count > 0 ? args[0] : undefined;
          try {
            converted = self->jsiToJSValue(rt, arg,
                                           /*unwrapThenables=*/!isReject);
          } catch (const jsi::JSError &e) {
            conversionError = e.getMessage();
          }

          if (converted) {
            JSValue *fn = isReject ? rejectFn : resolveFn;
            if (fn && !self->invokeSettlement((__bridge void *)fn, converted) &&
                !isReject && rejectFn) {
              // The resolve reaction loop was interrupted: still settle the
              // guest Promise with a clear timeout error.
              NSString *message = [NSString
                  stringWithFormat:@"JSC sandbox function timed out after "
                                   @"%lldms (execution interrupted)",
                                   static_cast<long long>(self->timeoutMs_)];
              JSValue *reason = [JSValue valueWithNewErrorFromMessage:message
                                                           inContext:ctx];
              self->invokeSettlement((__bridge void *)rejectFn,
                                     (__bridge void *)reason);
            }
          } else if (rejectFn) {
            // Still settle: reject with a real Error instead of leaving the
            // guest Promise pending.
            NSString *nsMessage =
                [NSString stringWithUTF8String:conversionError.c_str()];
            JSValue *reason = [JSValue
                valueWithNewErrorFromMessage:(nsMessage ?: @"")
                                   inContext:ctx];
            self->invokeSettlement((__bridge void *)rejectFn,
                                   (__bridge void *)reason);
          }

          if (ctx.exception) {
            ctx.exception = nil;
          }
          return jsi::Value::undefined();
        }
      });
}

std::shared_ptr<JSCSandboxContext::PromiseBridge>
JSCSandboxContext::makePromiseCallbacks(jsi::Runtime &rt, void *resolve,
                                        void *reject,
                                        jsi::Value &resolveProxy,
                                        jsi::Value &rejectProxy) {
  auto bridge = std::make_shared<PromiseBridge>();
  bridge->resolve = (__bridge JSValue *)resolve;
  bridge->reject = (__bridge JSValue *)reject;
  resolveProxy = makeSettlementProxy(rt, bridge, false);
  rejectProxy = makeSettlementProxy(rt, bridge, true);

  // Track in-flight pairs so dispose() can drop their guest references. The
  // weak list is pruned occasionally to stay bounded.
  std::lock_guard<std::recursive_mutex> lock(mutex_);
  if (promiseBridges_.size() > 64) {
    promiseBridges_.erase(
        std::remove_if(promiseBridges_.begin(), promiseBridges_.end(),
                       [](const std::weak_ptr<PromiseBridge> &entry) {
                         return entry.expired();
                       }),
        promiseBridges_.end());
  }
  promiseBridges_.push_back(bridge);
  return bridge;
}

// Convert JSValue* to jsi::Value (entry point, creates visited set for cycle detection)
jsi::Value JSCSandboxContext::jsValueToJSI(jsi::Runtime &rt, void *jsValue) {
  std::unordered_set<const void *> visited;
  return jsValueToJSI(rt, jsValue, 0, &visited);
}

// Depth-limited + cycle-detecting conversion to prevent stack overflow
// Depth limit guards against extremely deep (non-circular) structures.
// Visited set tracks the current ancestor path to detect true cycles (A->B->A).
// Important: entries are REMOVED after subtree conversion completes so that
// shared references between siblings (e.g., two operations sharing a style
// object) are NOT falsely flagged as circular.
//
// CRITICAL: Only track OBJECTS and ARRAYS in the visited set, NOT primitive values.
// JSC may reuse JSValueRef pointers for interned strings, small integers, or
// other optimized values. Tracking primitives would cause false positive
// circular reference detection when the same primitive value appears multiple
// times in an object tree.
static constexpr int kMaxConversionDepth = 64;

jsi::Value JSCSandboxContext::jsValueToJSI(jsi::Runtime &rt, void *jsValue,
                                            int depth,
                                            std::unordered_set<const void *> *visited) {
  JSValue *value = (__bridge JSValue *)jsValue;

  if (!value || [value isUndefined]) {
    return jsi::Value::undefined();
  }
  if ([value isNull]) {
    return jsi::Value::null();
  }
  if ([value isBoolean]) {
    return jsi::Value([value toBool]);
  }
  if ([value isNumber]) {
    return jsi::Value([value toDouble]);
  }
  if ([value isString]) {
    NSString *str = [value toString];
    return jsi::String::createFromUtf8(rt, [str UTF8String]);
  }

  // Depth guard for extremely deep (non-circular) structures
  if (depth >= kMaxConversionDepth) {
    NSLog(@"[JSCSandbox] jsValueToJSI: max depth %d reached, returning "
          @"undefined",
          kMaxConversionDepth);
    return jsi::Value::undefined();
  }

  // At this point, value must be an object or array (non-primitive).
  // Only track these composite types for circular reference detection.
  // Primitives (bool, number, string, null, undefined) are handled above
  // and should NOT be added to visited set as JSC may reuse their JSValueRef.
  JSValueRef valueRef = [value JSValueRef];
  bool addedToVisited = false;
  
  // DEBUG: Check if this is actually an object/array before adding to visited
  bool isRealObject = [value isObject] || [value isArray];
  
  if (visited && valueRef && isRealObject) {
    if (visited->count(valueRef)) {
      // Already visiting this exact object in current ancestor path - true cycle
      // DEBUG: Log more info about what we think is circular
      NSLog(@"[JSCSandbox] jsValueToJSI: circular reference detected at depth "
            @"%d, ptr=%p, isArray=%d, isObject=%d, visitedSize=%lu",
            depth, valueRef, [value isArray], [value isObject], visited->size());
      return jsi::Value::undefined();
    }
    visited->insert(valueRef);
    addedToVisited = true;
  }

  if ([value isArray]) {
    // Use valueAtIndex: to preserve JSValue identity (needed for cycle detection).
    // [value toArray] deep-converts to NSDictionary/NSArray, losing object identity.
    NSUInteger count = [[value[@"length"] toNumber] unsignedIntegerValue];
    jsi::Array jsiArr = jsi::Array(rt, count);
    for (NSUInteger i = 0; i < count; i++) {
      JSValue *elem = [value valueAtIndex:i];
      jsiArr.setValueAtIndex(
          rt, i, jsValueToJSI(rt, (__bridge void *)elem, depth + 1, visited));
    }
    // Remove from ancestor path: subtree is fully converted
    if (addedToVisited) visited->erase(valueRef);
    return std::move(jsiArr);
  }
  if ([value isObject]) {
    JSContext *ctx = (__bridge JSContext *)jsContext_;

    // Check if the value is a function FIRST
    // Use JavaScript's typeof to accurately detect functions
    JSValue *typeofResult =
        [ctx evaluateScript:@"(function(v) { return typeof v; })"];
    JSValue *typeStr = [typeofResult callWithArguments:@[ value ]];
    NSString *typeString = [typeStr toString];

    if ([typeString isEqualToString:@"function"]) {
      // Functions don't recurse into children, safe to remove from visited
      if (addedToVisited) visited->erase(valueRef);
      return makeFunctionProxy(rt, (__bridge void *)value);
    }

    // Binary passthrough — MUST come before the generic Object.keys copy,
    // which sees zero own enumerable props on an ArrayBuffer and would emit
    // an empty host object, silently destroying the bytes (the binary
    // op-batch failure mode). Bytes are COPIED into the host realm.
    {
      JSContextRef ctxRef = [ctx JSGlobalContextRef];
      JSValueRef probeExc = nullptr;
      JSTypedArrayType typedArrayType =
          JSValueGetTypedArrayType(ctxRef, valueRef, &probeExc);
      if (!probeExc && typedArrayType != kJSTypedArrayTypeNone) {
        JSValueRef exc = nullptr;
        JSObjectRef objRef = JSValueToObject(ctxRef, valueRef, &exc);
        if (!exc && objRef) {
          if (typedArrayType == kJSTypedArrayTypeArrayBuffer) {
            void *bytes = JSObjectGetArrayBufferBytesPtr(ctxRef, objRef, &exc);
            size_t byteLength =
                exc ? 0 : JSObjectGetArrayBufferByteLength(ctxRef, objRef, &exc);
            if (!exc) {
              if (addedToVisited) visited->erase(valueRef);
              return makeHostArrayBuffer(
                  rt, static_cast<const uint8_t *>(bytes), byteLength);
            }
          } else {
            // Typed-array view. Verified on-device (macOS 26 JSC):
            // JSObjectGetTypedArrayBytesPtr returns the BACKING ArrayBuffer's
            // start, NOT the view's data start — the view's byteOffset must
            // be applied explicitly or a subarray crosses with shifted bytes.
            void *bytes = JSObjectGetTypedArrayBytesPtr(ctxRef, objRef, &exc);
            size_t byteLength =
                exc ? 0 : JSObjectGetTypedArrayByteLength(ctxRef, objRef, &exc);
            size_t byteOffset =
                exc ? 0 : JSObjectGetTypedArrayByteOffset(ctxRef, objRef, &exc);
            if (!exc && bytes) {
              if (addedToVisited) visited->erase(valueRef);
              jsi::Value hostBuffer = makeHostArrayBuffer(
                  rt, static_cast<const uint8_t *>(bytes) + byteOffset,
                  byteLength);
              // Same-kind reconstruction, best-effort: a host realm without
              // the constructor still gets the raw bytes.
              const char *ctorName = typedArrayCtorName(typedArrayType);
              if (ctorName && rt.global().hasProperty(rt, ctorName)) {
                jsi::Value hostCtor = rt.global().getProperty(rt, ctorName);
                if (hostCtor.isObject() &&
                    hostCtor.getObject(rt).isFunction(rt)) {
                  try {
                    return hostCtor.getObject(rt)
                        .getFunction(rt)
                        .callAsConstructor(rt, hostBuffer);
                  } catch (...) {
                    // fall through to the raw ArrayBuffer
                  }
                }
              }
              return hostBuffer;
            }
          }
        }
        // Detached buffer / JSC error: fall through to the generic copy
        // (visited bookkeeping untouched — no early return happened).
      }
    }

    // Not a function, convert as regular object
    jsi::Object jsiObj = jsi::Object(rt);

    // Get all own property names using JavaScript
    JSValue *getKeysFunc =
        [ctx evaluateScript:@"(function(obj) { return Object.keys(obj); })"];
    JSValue *keysArray = [getKeysFunc callWithArguments:@[ value ]];

    if (keysArray && [keysArray isArray]) {
      NSArray *keys = [keysArray toArray];
      
      // DEBUG: Log object conversion at specific depth to track onPress issue
      if (depth <= 3) {
        NSLog(@"[JSCSandbox] jsValueToJSI object at depth %d: %lu keys", depth, (unsigned long)keys.count);
      }

      for (NSString *key in keys) {
        if (![key isKindOfClass:[NSString class]])
          continue;

        JSValue *propVal = value[key];
        if (!propVal || [propVal isUndefined])
          continue;

        // DEBUG: Track onPress conversion specifically
        if ([key isEqualToString:@"onPress"]) {
          NSLog(@"[JSCSandbox] jsValueToJSI: converting onPress property at depth %d, isObject=%d, isString=%d",
                depth, [propVal isObject], [propVal isString]);
          if ([propVal isObject]) {
            JSValue *typeField = propVal[@"__type"];
            NSLog(@"[JSCSandbox] jsValueToJSI: onPress.__type=%@",
                  [typeField isString] ? [typeField toString] : @"(not string)");
          }
        }

        // Recursively convert each property with depth + cycle tracking
        jsi::Value jsiPropVal =
            jsValueToJSI(rt, (__bridge void *)propVal, depth + 1, visited);
        
        // DEBUG: Track onPress conversion result
        if ([key isEqualToString:@"onPress"]) {
          NSLog(@"[JSCSandbox] jsValueToJSI: onPress converted at depth %d, result isUndefined=%d, isObject=%d",
                depth, jsiPropVal.isUndefined(), jsiPropVal.isObject());
        }
        
        jsiObj.setProperty(rt, [key UTF8String], std::move(jsiPropVal));
      }
    }

    // DEBUG: Log final object properties at depth 3 (where props object should be)
    if (depth == 3) {
      // Check what properties the converted object has
      jsi::Array propNames = jsiObj.getPropertyNames(rt);
      size_t propCount = propNames.size(rt);
      std::string propList;
      for (size_t i = 0; i < propCount && i < 10; i++) {
        if (i > 0) propList += ", ";
        propList += propNames.getValueAtIndex(rt, i).asString(rt).utf8(rt);
      }
      NSLog(@"[JSCSandbox] jsValueToJSI: depth %d object converted with %zu props: %s", depth, propCount, propList.c_str());
    }

    // Remove from ancestor path: subtree is fully converted
    if (addedToVisited) visited->erase(valueRef);
    return std::move(jsiObj);
  }

  return jsi::Value::undefined();
}

// MARK: - JSCSandboxRuntime Implementation

JSCSandboxRuntime::JSCSandboxRuntime(jsi::Runtime &hostRuntime, double timeout,
                                     bool enableExecutionTimeLimit)
    : hostRuntime_(&hostRuntime), timeout_(timeout),
      enableExecutionTimeLimit_(enableExecutionTimeLimit), disposed_(false) {}

JSCSandboxRuntime::~JSCSandboxRuntime() { dispose(); }

void JSCSandboxRuntime::dispose() {
  std::lock_guard<std::recursive_mutex> lock(mutex_);
  if (disposed_)
    return;
  disposed_ = true;

  for (auto &ctx : contexts_) {
    ctx->dispose();
  }
  contexts_.clear();
}

jsi::Value JSCSandboxRuntime::get(jsi::Runtime &rt,
                                  const jsi::PropNameID &name) {
  std::string propName = name.utf8(rt);

  if (propName == "createContext") {
    return jsi::Function::createFromHostFunction(
        rt, name, 0,
        [this](jsi::Runtime &rt, const jsi::Value &thisVal,
               const jsi::Value *args, size_t count) -> jsi::Value {
          (void)thisVal;
          (void)args;
          (void)count;
          return this->createContext(rt);
        });
  }

  if (propName == "dispose") {
    return jsi::Function::createFromHostFunction(
        rt, name, 0,
        [this](jsi::Runtime &rt, const jsi::Value &thisVal,
               const jsi::Value *args, size_t count) -> jsi::Value {
          (void)rt;
          (void)thisVal;
          (void)args;
          (void)count;
          this->dispose();
          return jsi::Value::undefined();
        });
  }

  return jsi::Value::undefined();
}

void JSCSandboxRuntime::set(jsi::Runtime &rt, const jsi::PropNameID &name,
                            const jsi::Value &value) {
  (void)rt;
  (void)name;
  (void)value;
  // Read-only
}

std::vector<jsi::PropNameID>
JSCSandboxRuntime::getPropertyNames(jsi::Runtime &rt) {
  std::vector<jsi::PropNameID> props;
  props.push_back(jsi::PropNameID::forUtf8(rt, "createContext"));
  props.push_back(jsi::PropNameID::forUtf8(rt, "dispose"));
  return props;
}

jsi::Value JSCSandboxRuntime::createContext(jsi::Runtime &rt) {
  std::lock_guard<std::recursive_mutex> lock(mutex_);

  if (disposed_) {
    throw jsi::JSError(rt, "Runtime has been disposed");
  }

  auto context = std::make_shared<JSCSandboxContext>(
      *hostRuntime_, timeout_, enableExecutionTimeLimit_);
  contexts_.push_back(context);

  return jsi::Object::createFromHostObject(rt, context);
}

// MARK: - JSCSandboxModule Implementation

JSCSandboxModule::JSCSandboxModule(jsi::Runtime &runtime) {
  (void)runtime; // Module does not need to store runtime reference
}

JSCSandboxModule::~JSCSandboxModule() {}

jsi::Value JSCSandboxModule::get(jsi::Runtime &rt,
                                 const jsi::PropNameID &name) {
  std::string propName = name.utf8(rt);

  if (propName == "createRuntime") {
    return jsi::Function::createFromHostFunction(
        rt, name, 1,
        [](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args,
           size_t count) -> jsi::Value {
          (void)thisVal;
          double timeout = 30000; // default 30s
          // Default OFF: enforcement uses a private JSC API (via dlsym) that
          // enterprise/internal builds may opt into; see header comment.
          bool enableExecutionTimeLimit = false;

          if (count > 0 && args[0].isObject()) {
            jsi::Object opts = args[0].asObject(rt);
            if (opts.hasProperty(rt, "timeout")) {
              jsi::Value timeoutVal = opts.getProperty(rt, "timeout");
              if (timeoutVal.isNumber()) {
                timeout = timeoutVal.getNumber();
              }
            }
            if (opts.hasProperty(rt, "enableExecutionTimeLimit")) {
              jsi::Value enableVal =
                  opts.getProperty(rt, "enableExecutionTimeLimit");
              if (enableVal.isBool()) {
                enableExecutionTimeLimit = enableVal.getBool();
              }
            }
          }

          auto runtime = std::make_shared<JSCSandboxRuntime>(
              rt, timeout, enableExecutionTimeLimit);
          return jsi::Object::createFromHostObject(rt, runtime);
        });
  }

  if (propName == "isAvailable") {
    return jsi::Function::createFromHostFunction(
        rt, name, 0,
        [](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args,
           size_t count) -> jsi::Value {
          (void)rt;
          (void)thisVal;
          (void)args;
          (void)count;
          return jsi::Value(true);
        });
  }

  return jsi::Value::undefined();
}

void JSCSandboxModule::set(jsi::Runtime &rt, const jsi::PropNameID &name,
                           const jsi::Value &value) {
  (void)rt;
  (void)name;
  (void)value;
  // Read-only
}

std::vector<jsi::PropNameID>
JSCSandboxModule::getPropertyNames(jsi::Runtime &rt) {
  std::vector<jsi::PropNameID> props;
  props.push_back(jsi::PropNameID::forUtf8(rt, "createRuntime"));
  props.push_back(jsi::PropNameID::forUtf8(rt, "isAvailable"));
  return props;
}

void JSCSandboxModule::install(jsi::Runtime &runtime) {
  auto module = std::make_shared<JSCSandboxModule>(runtime);
  jsi::Object moduleObj = jsi::Object::createFromHostObject(runtime, module);
  runtime.global().setProperty(runtime, "__JSCSandboxJSI",
                               std::move(moduleObj));
}

// Wrapper function for external linkage (avoids JSValue symbol conflicts)
void installJSCSandbox(jsi::Runtime &runtime) {
  JSCSandboxModule::install(runtime);
}

} // namespace jsc_sandbox
