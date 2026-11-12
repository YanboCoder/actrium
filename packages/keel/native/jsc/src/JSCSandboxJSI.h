#pragma once

#include <atomic>
#include <jsi/jsi.h>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <unordered_set>

namespace jsc_sandbox {

using namespace facebook;

/**
 * JSCSandboxContext - Wraps a single isolated JSContext
 *
 * Exposed to JS as a HostObject with SYNCHRONOUS methods:
 * - eval(code: string): unknown
 * - inject(name: string, value: unknown): void
 * - extract(name: string): unknown
 * - dispose(): void
 */
class JSCSandboxContext : public jsi::HostObject {
public:
  // `timeoutMs` is a wall-clock budget per top-level entry into sandbox JS.
  // It is only enforced when `enableExecutionTimeLimit` is true AND the
  // private JSC time-limit API could be resolved via dlsym (see .mm);
  // otherwise it is accepted but ignored.
  JSCSandboxContext(jsi::Runtime &hostRuntime, double timeoutMs,
                    bool enableExecutionTimeLimit);
  ~JSCSandboxContext() override;

  jsi::Value get(jsi::Runtime &rt, const jsi::PropNameID &name) override;
  void set(jsi::Runtime &rt, const jsi::PropNameID &name,
           const jsi::Value &value) override;
  std::vector<jsi::PropNameID> getPropertyNames(jsi::Runtime &rt) override;

  jsi::Value eval(jsi::Runtime &rt, const std::string &code);
  void inject(jsi::Runtime &rt, const std::string &name,
                 const jsi::Value &value);
  jsi::Value extract(jsi::Runtime &rt, const std::string &name);
  void dispose();

  bool isDisposed() const { return disposed_; }

private:
  /**
   * TimeLimitScope - RAII helper arming JSC's execution time limit for one
   * top-level entry into sandbox JS execution (eval or a host->sandbox
   * function call).
   *
   * - Only arms when the context resolved the private time-limit API
   *   (timeLimitEnabled_) and timeoutMs_ is a positive finite value that is
   *   safely representable; timeoutMs <= 0, NaN, Infinity or absurdly large
   *   budgets mean "no limit".
   * - Nested entries (host callback re-entering the sandbox during an eval)
   *   keep the OUTERMOST deadline: only the depth-0 scope arms, so a tenant
   *   cannot extend its budget by bouncing through host callbacks.
   */
  class TimeLimitScope {
  public:
    explicit TimeLimitScope(JSCSandboxContext &ctx);
    ~TimeLimitScope();
    TimeLimitScope(const TimeLimitScope &) = delete;
    TimeLimitScope &operator=(const TimeLimitScope &) = delete;

    // True if JSC aborted execution because THIS scope's deadline expired.
    bool timedOut() const;

  private:
    JSCSandboxContext &ctx_;
    bool armedHere_ = false;
  };

  // Identity-aware conversion state for host->guest conversion. Host objects
  // on the current conversion path map to the guest value already built for
  // them, so cycles reuse the same guest object instead of recursing.
  struct ConversionState;
  struct ConversionScope;
  // Shared ownership of one guest Promise's resolving functions. The native
  // side holds the only reference, so both entries are released as soon as
  // the Promise settles.
  struct PromiseBridge;

  void *jsContext_;    // JSContext* (opaque)
  void *contextGroup_; // JSContextGroupRef (opaque), owned by jsContext_'s VM
  jsi::Runtime *hostRuntime_;
  // Wall-clock execution budget per top-level entry; <= 0 means unlimited.
  double timeoutMs_;
  // True only when enableExecutionTimeLimit was requested AND the private
  // JSC API was resolved via dlsym. When false, timeouts are NOT enforced.
  bool timeLimitEnabled_;
  // Re-entrancy depth so nested host<->sandbox bounces keep the outermost
  // deadline (see TimeLimitScope).
  int timeLimitDepth_;
  // Set by the should-terminate callback when JSC aborted execution due to
  // the armed deadline; used to translate the generic termination exception
  // into a clear timeout error.
  std::atomic<bool> timeLimitFired_;
  bool disposed_;
  std::recursive_mutex mutex_;
  // Weak references to in-flight guest Promise callback pairs. dispose()
  // clears their strong guest references so none outlive the JSContext.
  std::vector<std::weak_ptr<PromiseBridge>> promiseBridges_;
  // JSValue* (retained) captured while the realm is pristine. Tenant code can
  // replace globalThis.Error/Promise, so failure-path helpers must keep their
  // own references instead of consulting guest globals.
  void *errorFactory_;
  void *promiseBridge_;

  // Callback storage for functions passed from host
  std::unordered_map<std::string, std::shared_ptr<jsi::Function>> callbacks_;
  
  // Cache for Host Function -> Proxy ID
  // Key: Unique identifier for the host function object (if available)
  // Since we can't easily get a stable ID from jsi::Function, we will use
  // a WeakMap-like approach on the JS side or simply try to cache by
  // hashing the function object if possible.
  //
  // BUT: In JSI, we can attach properties to HostObjects (if it is one),
  // but standard JS functions are opaque.
  //
  // Alternative: We can attach a hidden property to the host function
  // Reserved: callback counter was used for proxy ID assignment.
  // Kept as comment for reference; remove once proxy approach is finalized.

  void *jsiToJSValue(jsi::Runtime &rt, const jsi::Value &value,
                     bool unwrapThenables = true);
  void *jsiToJSValueImpl(jsi::Runtime &rt, const jsi::Value &value,
                         bool unwrapThenables, ConversionState &state);
  void *lookupConvertedObject(jsi::Runtime &rt, const jsi::Object &obj,
                              ConversionState &state);
  jsi::Value jsValueToJSI(jsi::Runtime &rt, void *jsValue);
  jsi::Value jsValueToJSI(jsi::Runtime &rt, void *jsValue, int depth,
                          std::unordered_set<const void *> *visited = nullptr);
  void *wrapFunctionForSandbox(jsi::Runtime &rt, jsi::Function &&func);
  // Store a guest function and return a host proxy that invokes it.
  jsi::Value makeFunctionProxy(jsi::Runtime &rt, void *fn);
  // Settlement proxies for one guest Promise. `resolveProxy` adopts
  // thenables, `rejectProxy` never does. Conversion failures reject the
  // guest Promise instead of escaping into the ignored host `.then` chain.
  std::shared_ptr<PromiseBridge> makePromiseCallbacks(
      jsi::Runtime &rt, void *resolve, void *reject, jsi::Value &resolveProxy,
      jsi::Value &rejectProxy);
  jsi::Value makeSettlementProxy(jsi::Runtime &rt,
                                 const std::shared_ptr<PromiseBridge> &bridge,
                                 bool isReject);
  // Invoke a guest resolving function under the sandbox execution budget.
  // Returns false when the budget fired; the context stays usable.
  bool invokeSettlement(void *fn, void *arg);
  // Lock the FIRST settlement attempt before any user-visible conversion.
  // On success both out parameters receive +1 references (the caller transfers
  // them with __bridge_transfer); re-entrant calls during conversion must not
  // win a later settlement. Returns false when already settled.
  bool beginSettlement(const std::shared_ptr<PromiseBridge> &bridge,
                       void **resolveOut, void **rejectOut);
  // Rebuild a host Error as a real sandbox Error. Returns nullptr when `obj`
  // is not a host Error.
  void *tryMakeSandboxError(jsi::Runtime &rt, const jsi::Object &obj,
                            ConversionState &state);
  // Convert the value thrown by the host into a sandbox rejection reason.
  // Rejection reasons are values, never thenables to adopt.
  void *sandboxRejectionReason(jsi::Runtime &rt, const jsi::JSError &error);
  // Settle a sandbox promise through its reject callback.
  void rejectSandboxPromise(void *rejectFn, void *reason);
};

/**
 * JSCSandboxRuntime - Factory for isolated contexts
 */
class JSCSandboxRuntime : public jsi::HostObject {
public:
  JSCSandboxRuntime(jsi::Runtime &hostRuntime, double timeout,
                    bool enableExecutionTimeLimit);
  ~JSCSandboxRuntime() override;

  jsi::Value get(jsi::Runtime &rt, const jsi::PropNameID &name) override;
  void set(jsi::Runtime &rt, const jsi::PropNameID &name,
           const jsi::Value &value) override;
  std::vector<jsi::PropNameID> getPropertyNames(jsi::Runtime &rt) override;

  jsi::Value createContext(jsi::Runtime &rt);
  void dispose();

private:
  jsi::Runtime *hostRuntime_;
  double timeout_;
  bool enableExecutionTimeLimit_;
  bool disposed_;
  std::vector<std::shared_ptr<JSCSandboxContext>> contexts_;
  std::recursive_mutex mutex_;
};

/**
 * JSCSandboxModule - Top-level JSI module
 *
 * Installed as global.__JSCSandboxJSI with:
 * - createRuntime(options?: { timeout?: number,
 *                             enableExecutionTimeLimit?: boolean }): Runtime
 *   timeout is a wall-clock budget in milliseconds applied to each top-level
 *   eval (default 30000). By default (enableExecutionTimeLimit false/absent)
 *   it is NOT enforced: JavaScriptCore's public API cannot interrupt running
 *   JS. When enableExecutionTimeLimit is explicitly true, the limit is
 *   enforced through the private JSContextGroupSetExecutionTimeLimit API
 *   resolved at runtime via dlsym (App Store review caveat — see .mm); a
 *   script exceeding it throws a timeout error and the context remains
 *   usable. timeout <= 0 disables the limit.
 * - isAvailable(): boolean
 */
class JSCSandboxModule : public jsi::HostObject {
public:
  explicit JSCSandboxModule(jsi::Runtime &runtime);
  ~JSCSandboxModule() override;

  jsi::Value get(jsi::Runtime &rt, const jsi::PropNameID &name) override;
  void set(jsi::Runtime &rt, const jsi::PropNameID &name,
           const jsi::Value &value) override;
  std::vector<jsi::PropNameID> getPropertyNames(jsi::Runtime &rt) override;

  static void install(jsi::Runtime &runtime);
};

} // namespace jsc_sandbox
