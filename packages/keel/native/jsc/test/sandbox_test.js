/**
 * JSC Sandbox Test Suite
 *
 * This test runs in the Host JS runtime and tests the sandbox functionality.
 * Both Host and Guest are JavaScript - this is the real usage scenario.
 */

(() => {
  // Skip test if JSI module is not available (e.g., running in Bun instead of React Native)
  if (typeof globalThis.__JSCSandboxJSI === 'undefined') {
    console.log('⊘ Skipping JSC native tests');
    console.log('  Reason: JSI module not available');
    console.log('  Environment: Non-React Native (Bun/Node.js)');
    console.log('  Platform: JSC only available on iOS/macOS/tvOS/visionOS');
    console.log('  To run: Use standalone RN test project at tests/rn-integration/');
    return;
  }

  var testsRun = 0;
  var testsPassed = 0;
  var testsFailed = 0;

  function assert(condition, testName, message) {
    testsRun++;
    if (condition) {
      testsPassed++;
      console.log(`  ✓ ${testName}`);
    } else {
      testsFailed++;
      console.log(`  ✗ ${testName}${message ? ` - ${message}` : ''}`);
    }
  }

  function assertThrows(fn, testName) {
    testsRun++;
    try {
      fn();
      testsFailed++;
      console.log(`  ✗ ${testName} - expected exception`);
    } catch (_e) {
      testsPassed++;
      console.log(`  ✓ ${testName}`);
    }
  }

  console.log('\n=== JSC Sandbox Tests (JavaScript) ===\n');

  // 1. Module Installation
  console.log('1. Module Installation');
  var sandbox = globalThis.__JSCSandboxJSI;
  assert(typeof sandbox === 'object', 'Module installed on global');
  assert(typeof sandbox.isAvailable === 'function', 'isAvailable is a function');
  assert(sandbox.isAvailable() === true, 'isAvailable() returns true');

  // 2. Runtime & Context Creation
  console.log('\n2. Runtime & Context Creation');
  var runtime = sandbox.createRuntime();
  assert(typeof runtime === 'object', 'createRuntime() returns object');

  var ctx = runtime.createContext();
  assert(typeof ctx === 'object', 'createContext() returns object');
  assert(typeof ctx.eval === 'function', 'context has eval method');
  assert(typeof ctx.inject === 'function', 'context has inject method');
  assert(typeof ctx.extract === 'function', 'context has extract method');

  // 3. Code Evaluation
  console.log('\n3. Code Evaluation');
  assert(ctx.eval('1 + 2') === 3, "eval('1 + 2') returns 3");
  assert(ctx.eval("'hello' + ' world'") === 'hello world', 'eval string concatenation');

  ctx.eval('function add(a, b) { return a + b; }');
  assert(ctx.eval('add(10, 20)') === 30, 'Define and call function');

  var arr = ctx.eval('[1, 2, 3].map(function(x) { return x * 2; })');
  assert(Array.isArray(arr), 'Array map returns array');
  assert(
    arr.length === 3 && arr[0] === 2 && arr[1] === 4 && arr[2] === 6,
    'Array map values correct'
  );

  // 4. inject / extract
  console.log('\n4. inject / extract');
  ctx.inject('myNumber', 42);
  assert(ctx.extract('myNumber') === 42, 'inject/extract number');
  assert(ctx.eval('myNumber') === 42, 'eval can access inject value');

  ctx.inject('myString', 'test string');
  assert(ctx.extract('myString') === 'test string', 'inject/extract string');

  ctx.inject('myObj', { x: 10, y: 20 });
  assert(ctx.eval('myObj.x + myObj.y') === 30, 'inject/extract object');

  ctx.inject('myArray', [1, 2, 3]);
  assert(
    ctx.eval('myArray.reduce(function(a, b) { return a + b; }, 0)') === 6,
    'inject/extract array'
  );

  // 5. Host Function Callbacks (the key feature!)
  console.log('\n5. Host Function Callbacks');

  var callbackInvoked = false;
  var receivedValue = 0;
  ctx.inject('hostCallback', (val) => {
    callbackInvoked = true;
    receivedValue = val;
    return val * 2;
  });

  var result = ctx.eval('hostCallback(21)');
  assert(
    callbackInvoked && receivedValue === 21 && result === 42,
    'Host function called from sandbox'
  );

  ctx.inject('multiArg', (a, b, c, d, e) => a + b + c + d + e);
  assert(ctx.eval('multiArg(1, 2, 3, 4, 5)') === 15, 'Host function with multiple args');

  var _receivedObj = null;
  ctx.inject('objCallback', (obj) => {
    _receivedObj = obj;
    return obj.name === 'Alice' && obj.age === 30;
  });
  assert(
    ctx.eval("objCallback({ name: 'Alice', age: 30 })") === true,
    'Host function receiving object'
  );

  // 6. Bidirectional: Guest function called by Host
  console.log('\n6. Guest Functions Callable from Host');
  ctx.eval('function guestAdd(a, b) { return a + b; }');
  var guestAdd = ctx.extract('guestAdd');
  assert(typeof guestAdd === 'function', 'Can get guest function');
  assert(guestAdd(5, 7) === 12, 'Can call guest function from host');

  // 7. Sandbox Isolation
  console.log('\n7. Sandbox Isolation');

  globalThis.hostGlobal = 100;
  ctx.eval('hostGlobal = 999'); // Guest tries to modify
  assert(globalThis.hostGlobal === 100, "Sandbox doesn't modify host globals");

  var ctx2 = runtime.createContext();
  ctx.eval("var sandboxVar = 'context1'");
  ctx2.eval("var sandboxVar = 'context2'");
  assert(ctx.eval('sandboxVar') === 'context1', 'Context 1 isolated');
  assert(ctx2.eval('sandboxVar') === 'context2', 'Context 2 isolated');
  ctx2.dispose();

  // 8. Error Handling
  console.log('\n8. Error Handling');
  assertThrows(() => {
    ctx.eval('function { invalid');
  }, 'Syntax error is caught');
  assertThrows(() => {
    ctx.eval("throw new Error('test error')");
  }, 'Runtime error is caught');

  // 9. Disposal
  console.log('\n9. Disposal');
  var tempCtx = runtime.createContext();
  tempCtx.eval('var x = 1');
  tempCtx.dispose();
  assertThrows(() => {
    tempCtx.eval('x + 1');
  }, 'Disposed context throws error');

  // 10. Primitive Types - null, undefined, boolean
  console.log('\n10. Primitive Types');
  ctx.inject('myNull', null);
  assert(ctx.extract('myNull') === null, 'inject/extract null');
  assert(ctx.eval('myNull === null') === true, 'eval null comparison');

  ctx.inject('myUndef', undefined);
  assert(ctx.extract('myUndef') === undefined, 'inject/extract undefined');
  assert(ctx.eval('myUndef === undefined') === true, 'eval undefined comparison');

  ctx.inject('myTrue', true);
  ctx.inject('myFalse', false);
  assert(ctx.extract('myTrue') === true, 'inject/extract true');
  assert(ctx.extract('myFalse') === false, 'inject/extract false');
  assert(ctx.eval('myTrue && !myFalse') === true, 'eval boolean logic');

  // 11. Special Numbers - NaN, Infinity
  console.log('\n11. Special Numbers');
  ctx.inject('myNaN', NaN);
  assert(Number.isNaN(ctx.extract('myNaN')), 'inject/extract NaN');
  assert(ctx.eval('Number.isNaN(myNaN)') === true, 'eval NaN check');

  ctx.inject('myInf', Infinity);
  assert(ctx.extract('myInf') === Infinity, 'inject/extract Infinity');
  assert(ctx.eval('myInf === Infinity') === true, 'eval Infinity comparison');

  ctx.inject('myNegInf', -Infinity);
  assert(ctx.extract('myNegInf') === -Infinity, 'inject/extract -Infinity');

  // 12. BigInt (if supported)
  console.log('\n12. BigInt');
  try {
    // Test BigInt operations within sandbox (not crossing boundary)
    assert(ctx.eval('typeof BigInt') === 'function', 'BigInt function exists in sandbox');
    assert(ctx.eval('typeof BigInt(123)') === 'bigint', 'BigInt type in sandbox');
    assert(
      ctx.eval('BigInt(10) + BigInt(20) === BigInt(30)') === true,
      'BigInt arithmetic in sandbox'
    );
    // Test that BigInt preserves precision beyond Number.MAX_SAFE_INTEGER
    assert(
      ctx.eval("BigInt('9007199254740993') > BigInt('9007199254740992')") === true,
      'BigInt large number comparison'
    );
  } catch (e) {
    console.log(`  ⚠ BigInt tests skipped: ${e.message}`);
  }

  // 13. Nested Objects
  console.log('\n13. Nested Objects');
  var nested = {
    level1: {
      level2: {
        level3: {
          value: 'deep',
        },
      },
    },
  };
  ctx.inject('nested', nested);
  assert(ctx.eval('nested.level1.level2.level3.value') === 'deep', 'Deep nested object access');

  var deepResult = ctx.extract('nested');
  assert(deepResult.level1.level2.level3.value === 'deep', 'extract deep nested object');

  // 14. Arrays with Mixed Types
  console.log('\n14. Arrays with Mixed Types');
  var mixedArray = [1, 'two', true, null, undefined, { x: 10 }, [1, 2, 3]];
  ctx.inject('mixedArray', mixedArray);
  assert(ctx.eval('mixedArray[0]') === 1, 'Mixed array - number');
  assert(ctx.eval('mixedArray[1]') === 'two', 'Mixed array - string');
  assert(ctx.eval('mixedArray[2]') === true, 'Mixed array - boolean');
  assert(ctx.eval('mixedArray[3] === null') === true, 'Mixed array - null');
  assert(ctx.eval('mixedArray[4] === undefined') === true, 'Mixed array - undefined');
  assert(ctx.eval('mixedArray[5].x') === 10, 'Mixed array - object');
  assert(ctx.eval('mixedArray[6][1]') === 2, 'Mixed array - nested array');

  // 15. Functions as Values
  console.log('\n15. Functions as Values');
  ctx.eval("var objWithFunc = { greet: function(name) { return 'Hello, ' + name; } }");
  var objWithFunc = ctx.extract('objWithFunc');
  assert(typeof objWithFunc.greet === 'function', 'Object with function property');
  assert(objWithFunc.greet('World') === 'Hello, World', 'Call function from object');

  ctx.eval('var higherOrder = function(f, x) { return f(x * 2); }');
  var higherOrder = ctx.extract('higherOrder');
  assert(higherOrder((n) => n + 1, 5) === 11, 'Higher-order function');

  // 16. Host Callback with Various Return Types
  console.log('\n16. Host Callback Return Types');
  ctx.inject('returnNull', () => null);
  assert(ctx.eval('returnNull() === null') === true, 'Host callback returning null');

  ctx.inject('returnUndefined', () => undefined);
  assert(ctx.eval('returnUndefined() === undefined') === true, 'Host callback returning undefined');

  ctx.inject('returnBool', (b) => !b);
  assert(ctx.eval('returnBool(false)') === true, 'Host callback returning boolean');

  ctx.inject('returnArray', () => [1, 2, 3]);
  var retArr = ctx.eval('returnArray()');
  assert(Array.isArray(retArr) && retArr.length === 3, 'Host callback returning array');

  ctx.inject('returnObject', () => ({ a: 1, b: 2 }));
  assert(ctx.eval('returnObject().a + returnObject().b') === 3, 'Host callback returning object');

  // 17. Guest Callback Receiving Various Types
  console.log('\n17. Guest Callback Arguments');
  ctx.eval('function identity(x) { return x; }');
  var identity = ctx.extract('identity');
  assert(identity(null) === null, 'Guest function receives null');
  assert(identity(undefined) === undefined, 'Guest function receives undefined');
  assert(identity(true) === true, 'Guest function receives boolean');
  assert(identity(3.14) === 3.14, 'Guest function receives float');
  var identObj = identity({ test: 123 });
  assert(identObj && identObj.test === 123, 'Guest function receives object');

  // 18. Error Objects
  console.log('\n18. Error Objects');
  try {
    ctx.eval("throw new TypeError('custom type error')");
    assert(false, 'Should have thrown');
  } catch (e) {
    assert(
      e.message && e.message.indexOf('custom type error') !== -1,
      'TypeError caught with message'
    );
  }

  try {
    ctx.eval("throw new RangeError('out of range')");
    assert(false, 'Should have thrown');
  } catch (e) {
    assert(e.message && e.message.indexOf('out of range') !== -1, 'RangeError caught with message');
  }

  // 19. Date Objects
  console.log('\n19. Date Objects');
  ctx.eval('var myDate = new Date(2024, 0, 15)');
  var dateObj = ctx.extract('myDate');
  assert(dateObj instanceof Date || typeof dateObj === 'object', 'Date object retrieved');
  assert(ctx.eval('myDate.getFullYear()') === 2024, 'Date getFullYear');
  assert(ctx.eval('myDate.getMonth()') === 0, 'Date getMonth');
  assert(ctx.eval('myDate.getDate()') === 15, 'Date getDate');

  // 20. RegExp Objects
  console.log('\n20. RegExp Objects');
  ctx.eval('var myRegex = /hello\\s+world/i');
  assert(ctx.eval("myRegex.test('Hello World')") === true, 'RegExp test match');
  assert(ctx.eval("myRegex.test('goodbye')") === false, 'RegExp test no match');
  assert(ctx.eval("'Hello   World'.match(myRegex) !== null") === true, 'RegExp match');

  // 21. Map and Set (if supported)
  console.log('\n21. Map and Set');
  try {
    ctx.eval("var myMap = new Map(); myMap.set('key1', 'value1'); myMap.set('key2', 42);");
    assert(ctx.eval("myMap.get('key1')") === 'value1', 'Map get string value');
    assert(ctx.eval("myMap.get('key2')") === 42, 'Map get number value');
    assert(ctx.eval('myMap.size') === 2, 'Map size');

    ctx.eval('var mySet = new Set([1, 2, 3, 2, 1]);');
    assert(ctx.eval('mySet.size') === 3, 'Set size (duplicates removed)');
    assert(ctx.eval('mySet.has(2)') === true, 'Set has');
    assert(ctx.eval('mySet.has(5)') === false, 'Set has (not present)');
  } catch (e) {
    console.log(`  ⚠ Map/Set tests skipped: ${e.message}`);
  }

  // 22. TypedArrays (if supported)
  console.log('\n22. TypedArrays');
  try {
    ctx.eval('var uint8 = new Uint8Array([1, 2, 3, 255])');
    assert(ctx.eval('uint8.length') === 4, 'Uint8Array length');
    assert(ctx.eval('uint8[0]') === 1, 'Uint8Array element access');
    assert(ctx.eval('uint8[3]') === 255, 'Uint8Array max value');

    ctx.eval('var int32 = new Int32Array([100, -100, 2147483647])');
    assert(ctx.eval('int32.length') === 3, 'Int32Array length');
    assert(ctx.eval('int32[1]') === -100, 'Int32Array negative value');

    ctx.eval('var float64 = new Float64Array([1.5, 2.5, 3.14159])');
    assert(
      ctx.eval('float64[2]') > 3.14 && ctx.eval('float64[2]') < 3.15,
      'Float64Array precision'
    );
  } catch (e) {
    console.log(`  ⚠ TypedArray tests skipped: ${e.message}`);
  }

  // 23. Symbol (if supported)
  console.log('\n23. Symbol');
  try {
    ctx.eval("var sym1 = Symbol('test'); var sym2 = Symbol('test');");
    assert(ctx.eval('sym1 !== sym2') === true, 'Symbols are unique');
    assert(ctx.eval('typeof sym1') === 'symbol', 'typeof Symbol');

    ctx.eval("var symObj = {}; symObj[sym1] = 'symbol value';");
    assert(ctx.eval('symObj[sym1]') === 'symbol value', 'Symbol as object key');
  } catch (e) {
    console.log(`  ⚠ Symbol tests skipped: ${e.message}`);
  }

  // 24. Promise (basic, if supported)
  console.log('\n24. Promise (basic)');
  try {
    ctx.eval('var promiseResolved = false; var promiseValue = 0;');
    ctx.eval(
      'Promise.resolve(42).then(function(v) { promiseResolved = true; promiseValue = v; });'
    );
    // Note: Promises are async, so we can't directly test the resolved value synchronously
    // But we can test that Promise exists and basic operations work
    assert(ctx.eval('typeof Promise') === 'function', 'Promise exists');
    assert(ctx.eval('typeof Promise.resolve') === 'function', 'Promise.resolve exists');
    assert(ctx.eval('typeof Promise.reject') === 'function', 'Promise.reject exists');
  } catch (e) {
    console.log(`  ⚠ Promise tests skipped: ${e.message}`);
  }

  // 24b. Host thenable conversion
  console.log('\n24b. Host thenable conversion');
  var thenableCalls = 0;
  var hostThenable = {
    value: 42,
    // biome-ignore lint/suspicious/noThenProperty: intentionally models a host thenable for the bridge regression.
    then: function (onFulfilled) {
      thenableCalls++;
      onFulfilled(this.value);
    },
  };
  ctx.inject('hostThenable', hostThenable);
  assert(
    ctx.eval(
      'hostThenable.then(function(v) { globalThis.resolvedValue = v; }) instanceof Promise'
    ) === true,
    'Host thenable becomes a real Guest Promise'
  );
  assert(ctx.eval('resolvedValue') === 42, 'Guest resolver receives the fulfilled value');
  assert(thenableCalls === 1, 'Host thenable keeps its original receiver');

  ctx.inject('realHostPromise', Promise.resolve(42));
  assert(
    ctx.eval('realHostPromise instanceof Promise') === true,
    'Host Promise preserves its prototype through conversion'
  );

  // 24c. Host thenable failures and rejections
  console.log('\n24c. Host thenable failures and rejections');
  var throwingThenable = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally models a host thenable that throws synchronously.
    then: function () {
      throw new Error('thenable-boom');
    },
  };
  ctx.inject('throwingThenable', throwingThenable);
  assert(
    ctx.eval('throwingThenable instanceof Promise') === true,
    'Throwing host thenable still becomes a Guest Promise'
  );
  ctx.eval(
    'globalThis.thenableRejection = null; throwingThenable.catch(function(e) { globalThis.thenableRejection = e; });'
  );
  assert(
    ctx.eval('thenableRejection instanceof Error') === true,
    'Throwing thenable rejects with a Guest Error'
  );
  assert(
    ctx.eval('thenableRejection.message') === 'thenable-boom',
    'Throwing thenable preserves the rejection message'
  );

  var hostRejection = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally models a host capability rejecting a guest handler.
    then: function (onFulfilled, onRejected) {
      onRejected(new TypeError('host-boom'));
    },
  };
  ctx.inject('hostRejection', hostRejection);
  assert(
    ctx.eval('hostRejection instanceof Promise') === true,
    'Rejected host thenable becomes a Guest Promise'
  );
  ctx.eval(
    'globalThis.hostRejectionResult = null; hostRejection.catch(function(e) { globalThis.hostRejectionResult = e; });'
  );
  assert(
    ctx.eval('hostRejectionResult instanceof Error') === true,
    'Host rejection reason is rebuilt as a Guest Error'
  );
  assert(
    ctx.eval('hostRejectionResult instanceof TypeError') === true,
    'Host rejection reason keeps its Error subtype'
  );
  assert(
    ctx.eval('hostRejectionResult ? hostRejectionResult.message : null') === 'host-boom',
    'Host rejection reason keeps its message'
  );
  assert(
    ctx.eval(
      'hostRejectionResult && typeof hostRejectionResult.stack === "string" && hostRejectionResult.stack.length > 0'
    ) === true,
    'Host rejection reason keeps its stack'
  );

  // Physical layout of the review scenarios: a throwing `then` getter, a
  // non-Error thrown value, and settle-then-throw.
  var throwingGetter = {};
  Object.defineProperty(throwingGetter, 'then', {
    get: function () {
      throw new RangeError('getter-boom');
    },
  });
  ctx.inject('throwingGetter', throwingGetter);
  ctx.eval(
    'globalThis.getterRejection = null; throwingGetter.catch(function(e) { globalThis.getterRejection = e; });'
  );
  assert(
    ctx.eval('getterRejection instanceof RangeError') === true,
    'Throwing then getter keeps the rejection Error subtype'
  );
  assert(
    ctx.eval('getterRejection ? getterRejection.message : null') === 'getter-boom',
    'Throwing then getter keeps the rejection message'
  );

  var throwingPrimitive = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally models a host thenable throwing a non-Error value.
    then: function () {
      throw 'primitive-boom';
    },
  };
  ctx.inject('throwingPrimitive', throwingPrimitive);
  ctx.eval(
    'globalThis.primitiveRejection = null; throwingPrimitive.catch(function(e) { globalThis.primitiveRejection = e; });'
  );
  assert(
    ctx.eval("primitiveRejection === 'primitive-boom'") === true,
    'Thrown primitive stays the rejection reason'
  );

  var settleThenThrow = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally models a thenable that settles then throws.
    then: function (onFulfilled) {
      onFulfilled(7);
      throw new Error('late-boom');
    },
  };
  ctx.inject('settleThenThrow', settleThenThrow);
  ctx.eval(
    'globalThis.settledValue = null; globalThis.settledRejection = null;' +
      'settleThenThrow.then(function(v) { globalThis.settledValue = v; }, function(e) { globalThis.settledRejection = e; });'
  );
  assert(ctx.eval('settledValue') === 7, 'First settlement wins when then settles then throws');
  assert(ctx.eval('settledRejection') === null, 'Late throw after settlement is ignored');

  // Tenant code can replace globalThis.Error; the rebuild path must keep its
  // captured realm intrinsics and must not leave a pending exception behind.
  ctx.eval('globalThis.__savedError__ = Error; globalThis.Error = undefined;');
  var throwingWithGuardedError = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally models a host thenable rejected after the guest removed Error.
    then: function () {
      throw new Error('guarded-boom');
    },
  };
  ctx.inject('throwingWithGuardedError', throwingWithGuardedError);
  ctx.eval(
    'globalThis.guardedRejection = null; throwingWithGuardedError.catch(function(e) { globalThis.guardedRejection = e; });'
  );
  assert(
    ctx.eval('guardedRejection ? guardedRejection.message : null') === 'guarded-boom',
    'Rejection reason survives a guest-replaced global Error'
  );
  assert(
    ctx.eval('globalThis.afterGuardedInjection = 123; 1 + 1') === 2,
    'Context stays usable after a guest-replaced global Error'
  );
  assert(
    ctx.eval('afterGuardedInjection') === 123,
    'No stale guest exception after the guarded path'
  );
  ctx.eval('globalThis.Error = globalThis.__savedError__; delete globalThis.__savedError__;');

  // The Error brand is decided by the HOST prototype chain, not by
  // Symbol.toStringTag, and `name` is copied as data, never used as the type.
  var spoofedError = { message: 'plain-data', payload: 42 };
  Object.defineProperty(spoofedError, Symbol.toStringTag, { value: 'Error' });
  ctx.inject('spoofedError', spoofedError);
  assert(
    ctx.eval('spoofedError.payload') === 42 && ctx.eval('spoofedError instanceof Error') === false,
    'Object spoofing the Error tag stays a plain guest object'
  );

  var renamedError = new Error('renamed-boom');
  renamedError.name = 'TypeError';
  ctx.inject('renamedError', renamedError);
  assert(
    ctx.eval('renamedError instanceof Error && !(renamedError instanceof TypeError)') === true,
    'Renamed base Error keeps its real prototype'
  );
  assert(
    ctx.eval('renamedError.name') === 'TypeError',
    'Renamed base Error keeps the name field as data'
  );

  // Custom enumerable Error fields must survive the rebuild instead of being
  // dropped by the new fast path.
  var codedRejection = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally rejects with a coded Error.
    then: function (onFulfilled, onRejected) {
      onRejected(Object.assign(new TypeError('coded-boom'), { code: 'E_X' }));
    },
  };
  ctx.inject('codedRejection', codedRejection);
  ctx.eval(
    'globalThis.codedResult = null; codedRejection.catch(function(e) { globalThis.codedResult = e; });'
  );
  assert(
    ctx.eval('codedResult instanceof TypeError') === true,
    'Coded rejection keeps its Error subtype'
  );
  assert(
    ctx.eval('codedResult ? codedResult.code : null') === 'E_X',
    'Coded rejection keeps custom own fields'
  );

  // A rejection reason that is itself a thenable is a value, not a promise to
  // adopt: the guest must receive the object with its fields intact.
  var thenableReasonCalls = 0;
  var thenableReason = {
    code: 'E_T',
    // biome-ignore lint/suspicious/noThenProperty: intentionally models a thenable rejection reason.
    then: function () {
      thenableReasonCalls++;
    },
  };
  var rejectingWithThenableReason = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally rejects with a thenable reason.
    then: function (onFulfilled, onRejected) {
      onRejected(thenableReason);
    },
  };
  ctx.inject('rejectingWithThenableReason', rejectingWithThenableReason);
  ctx.eval(
    'globalThis.thenableReasonResult = null; rejectingWithThenableReason.catch(function(e) { globalThis.thenableReasonResult = e; });'
  );
  assert(
    ctx.eval('thenableReasonResult instanceof Promise') === false,
    'Thenable rejection reason is not adopted into a Promise'
  );
  assert(
    ctx.eval('thenableReasonResult ? thenableReasonResult.code : null') === 'E_T',
    'Thenable rejection reason keeps its fields'
  );
  assert(thenableReasonCalls === 0, 'Thenable rejection reason is not invoked');

  // 24d. Cyclic host object conversion
  console.log('\n24d. Cyclic host object conversion');
  var cyclicError = new TypeError('cycle-boom');
  cyclicError.self = cyclicError;
  ctx.inject('cyclicError', cyclicError);
  assert(ctx.eval('cyclicError instanceof TypeError') === true, 'Cyclic Error keeps its subtype');
  assert(
    ctx.eval('cyclicError.self === cyclicError') === true,
    'Direct Error self-cycle preserves identity'
  );

  var cyclicErrorA = new Error('cycle-a');
  var cyclicErrorB = new Error('cycle-b');
  cyclicErrorA.peer = cyclicErrorB;
  cyclicErrorB.peer = cyclicErrorA;
  ctx.inject('cyclicErrorPair', cyclicErrorA);
  assert(
    ctx.eval('cyclicErrorPair.peer.peer === cyclicErrorPair') === true,
    'Indirect Error cycle preserves identity'
  );

  var cyclicOwner = new Error('owner-boom');
  var cyclicContainer = { owner: cyclicOwner };
  cyclicOwner.container = cyclicContainer;
  ctx.inject('cyclicOwner', cyclicOwner);
  assert(
    ctx.eval('cyclicOwner.container.owner === cyclicOwner') === true,
    'Error -> object -> Error cycle preserves identity'
  );

  var cyclicPlainA = {};
  var cyclicPlainB = { back: cyclicPlainA };
  cyclicPlainA.forward = cyclicPlainB;
  ctx.inject('cyclicPlain', cyclicPlainA);
  assert(
    ctx.eval('cyclicPlain.forward.back === cyclicPlain') === true,
    'Plain object cycle preserves identity'
  );

  var cyclicArray = [];
  cyclicArray.push(cyclicArray);
  ctx.inject('cyclicArray', cyclicArray);
  assert(
    ctx.eval('cyclicArray[0] === cyclicArray') === true,
    'Array self-cycle preserves identity'
  );

  // 24e. Re-entrant settlement during conversion
  console.log('\n24e. Re-entrant settlement during conversion');
  var savedReject = null;
  var reentrantResolveValue = {
    get detail() {
      if (savedReject) {
        savedReject('late-reject');
      }
      return 'outer-value';
    },
  };
  var reentrantResolveThenable = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally re-enters the reject proxy from a getter.
    then: function (onFulfilled, onRejected) {
      savedReject = onRejected;
      onFulfilled(reentrantResolveValue);
    },
  };
  ctx.inject('reentrantResolve', reentrantResolveThenable);
  ctx.eval(
    'globalThis.reentrantResolveOutcome = null; reentrantResolve.then(function(v) { globalThis.reentrantResolveOutcome = v; });'
  );
  assert(
    ctx.eval('reentrantResolveOutcome ? reentrantResolveOutcome.detail : null') === 'outer-value',
    'Re-entrant reject during resolve conversion is ignored'
  );

  var savedResolve = null;
  var reentrantRejectValue = {
    get detail() {
      if (savedResolve) {
        savedResolve('late-resolve');
      }
      return 'outer-reject';
    },
  };
  var reentrantRejectThenable = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally re-enters the resolve proxy from a getter.
    then: function (onFulfilled, onRejected) {
      savedResolve = onFulfilled;
      onRejected(reentrantRejectValue);
    },
  };
  ctx.inject('reentrantReject', reentrantRejectThenable);
  ctx.eval(
    'globalThis.reentrantRejectOutcome = null; reentrantReject.catch(function(e) { globalThis.reentrantRejectOutcome = e; });'
  );
  assert(
    ctx.eval('reentrantRejectOutcome ? reentrantRejectOutcome.detail : null') === 'outer-reject',
    'Re-entrant resolve during reject conversion is ignored'
  );

  var savedThrowReject = null;
  var syncThrowReason = {
    get detail() {
      if (savedThrowReject) {
        savedThrowReject('late-throw-reject');
      }
      return 'throw-reason';
    },
  };
  var syncThrowReentrantThenable = {
    // biome-ignore lint/suspicious/noThenProperty: intentionally re-enters the reject proxy while the thrown reason converts.
    then: function (onFulfilled, onRejected) {
      savedThrowReject = onRejected;
      throw syncThrowReason;
    },
  };
  ctx.inject('syncThrowReentrant', syncThrowReentrantThenable);
  ctx.eval(
    'globalThis.syncThrowOutcome = null; syncThrowReentrant.catch(function(e) { globalThis.syncThrowOutcome = e; });'
  );
  assert(
    ctx.eval('syncThrowOutcome ? syncThrowOutcome.detail : null') === 'throw-reason',
    'Re-entrant settlement during thrown-reason conversion is ignored'
  );

  // A real host Promise settles on the host microtask queue, which only
  // drains after this script returns to the runtime. The runner executes
  // __jscSandboxTestPhase2 in a second evaluation to observe it.
  var realRejected = Promise.reject(new TypeError('async-host-boom'));
  realRejected.catch(function () {});
  ctx.inject('realRejected', realRejected);
  ctx.eval('realRejected.catch(function () {});');

  var realThenableReasonCalls = 0;
  var realThenableReason = {
    code: 'E_ASYNC_T',
    // biome-ignore lint/suspicious/noThenProperty: intentionally models an async thenable rejection reason.
    then: function () {
      realThenableReasonCalls++;
    },
  };
  var realThenableRejected = Promise.reject(realThenableReason);
  realThenableRejected.catch(function () {});
  ctx.inject('realThenableRejected', realThenableRejected);
  ctx.eval('realThenableRejected.catch(function () {});');

  // Async conversion failures must settle the guest Promise instead of
  // leaving it pending, and settled Promises must not root callback entries.
  var throwingDetail = {
    get detail() {
      throw new Error('detail-boom');
    },
  };
  var failingResolve = Promise.resolve(throwingDetail);
  failingResolve.catch(function () {});
  ctx.inject('failingResolve', failingResolve);
  ctx.eval('failingResolve.catch(function () {});');

  var failingReject = Promise.reject(throwingDetail);
  failingReject.catch(function () {});
  ctx.inject('failingReject', failingReject);
  ctx.eval('failingReject.catch(function () {});');

  var callbackGlobalsBefore = ctx.eval(
    'Object.keys(globalThis).filter(function(k) { return k.indexOf("__sandbox_fn_") === 0; }).length'
  );
  for (var leakProbe = 0; leakProbe < 5; leakProbe++) {
    ctx.inject('leakProbe' + leakProbe, Promise.resolve(leakProbe));
  }

  globalThis.__jscSandboxTestPhase2 = function () {
    ctx.eval(
      'globalThis.realRejection = null; realRejected.catch(function(e) { globalThis.realRejection = e; });'
    );
    assert(
      ctx.eval('realRejection instanceof Error') === true,
      'Real host Promise rejection arrives as a Guest Error'
    );
    assert(
      ctx.eval('realRejection instanceof TypeError') === true,
      'Real host Promise rejection keeps its Error subtype'
    );
    assert(
      ctx.eval('realRejection ? realRejection.message : null') === 'async-host-boom',
      'Real host Promise rejection keeps its message'
    );
    assert(
      ctx.eval(
        'realRejection && typeof realRejection.stack === "string" && realRejection.stack.length > 0'
      ) === true,
      'Real host Promise rejection keeps its stack'
    );
    ctx.eval(
      'globalThis.realThenableReasonResult = null; realThenableRejected.catch(function(e) { globalThis.realThenableReasonResult = e; });'
    );
    assert(
      ctx.eval('realThenableReasonResult instanceof Promise') === false,
      'Real async thenable reason is not adopted into a Promise'
    );
    assert(
      ctx.eval('realThenableReasonResult ? realThenableReasonResult.code : null') === 'E_ASYNC_T',
      'Real async thenable reason keeps its fields'
    );
    assert(realThenableReasonCalls === 0, 'Real async thenable reason is not invoked');
    ctx.eval(
      'globalThis.failingResolveResult = "pending"; failingResolve.then(' +
        'function() { globalThis.failingResolveResult = "fulfilled"; },' +
        'function(e) { globalThis.failingResolveResult = e; });'
    );
    assert(
      ctx.eval('failingResolveResult !== "pending"') === true,
      'Async resolve conversion failure settles the guest Promise'
    );
    assert(
      ctx.eval('failingResolveResult instanceof Error') === true,
      'Async resolve conversion failure rejects with a Guest Error'
    );
    ctx.eval(
      'globalThis.failingRejectResult = "pending"; failingReject.then(' +
        'function() { globalThis.failingRejectResult = "fulfilled"; },' +
        'function(e) { globalThis.failingRejectResult = e; });'
    );
    assert(
      ctx.eval('failingRejectResult !== "pending"') === true,
      'Async reject conversion failure settles the guest Promise'
    );
    assert(
      ctx.eval('failingRejectResult instanceof Error') === true,
      'Async reject conversion failure rejects with a Guest Error'
    );
    var callbackGlobalsAfter = ctx.eval(
      'Object.keys(globalThis).filter(function(k) { return k.indexOf("__sandbox_fn_") === 0; }).length'
    );
    assert(
      callbackGlobalsAfter === callbackGlobalsBefore,
      'Settled host Promises leave no global callback entries'
    );
    assert(
      promiseTimeoutCtx.eval('1 + 1') === 2,
      'Promise settlement stays inside the execution budget'
    );
    promiseTimeoutRt.dispose();
    console.log('\n=== Deferred Test Summary ===');
    console.log(`Total: ${testsRun}`);
    console.log(`Passed: ${testsPassed}`);
    console.log(`Failed: ${testsFailed}`);
    return testsFailed === 0;
  };

  // 25. JSON serialization
  console.log('\n25. JSON Serialization');
  var jsonObj = { name: 'test', count: 42, active: true, items: [1, 2, 3] };
  ctx.inject('jsonObj', jsonObj);
  var jsonStr = ctx.eval('JSON.stringify(jsonObj)');
  assert(typeof jsonStr === 'string', 'JSON.stringify returns string');
  var parsed = JSON.parse(jsonStr);
  assert(parsed.name === 'test' && parsed.count === 42, 'JSON roundtrip preserves data');

  ctx.eval('var parsedInGuest = JSON.parse(\'{"x": 100, "y": 200}\')');
  assert(ctx.eval('parsedInGuest.x + parsedInGuest.y') === 300, 'JSON.parse in guest');

  // 26. Object with prototype methods
  console.log('\n26. Object Methods');
  ctx.eval('var arr = [3, 1, 4, 1, 5, 9, 2, 6]');
  assert(ctx.eval("arr.sort().join(',')") === '1,1,2,3,4,5,6,9', 'Array sort and join');
  assert(ctx.eval('arr.filter(function(x) { return x > 3; }).length') === 4, 'Array filter');
  assert(ctx.eval('arr.find(function(x) { return x > 5; })') === 6, 'Array find');
  assert(ctx.eval('arr.every(function(x) { return x > 0; })') === true, 'Array every');
  assert(ctx.eval('arr.some(function(x) { return x > 8; })') === true, 'Array some');

  // 27. String methods
  console.log('\n27. String Methods');
  ctx.inject('testStr', '  Hello, World!  ');
  assert(ctx.eval('testStr.trim()') === 'Hello, World!', 'String trim');
  assert(ctx.eval('testStr.toUpperCase().trim()') === 'HELLO, WORLD!', 'String toUpperCase');
  assert(ctx.eval('testStr.toLowerCase().trim()') === 'hello, world!', 'String toLowerCase');
  assert(ctx.eval("testStr.includes('World')") === true, 'String includes');
  assert(ctx.eval("testStr.indexOf('World')") === 9, 'String indexOf');
  assert(ctx.eval("testStr.split(',').length") === 2, 'String split');

  // 28. Math operations
  console.log('\n28. Math Operations');
  assert(ctx.eval('Math.floor(3.7)') === 3, 'Math.floor');
  assert(ctx.eval('Math.ceil(3.2)') === 4, 'Math.ceil');
  assert(ctx.eval('Math.round(3.5)') === 4, 'Math.round');
  assert(ctx.eval('Math.abs(-5)') === 5, 'Math.abs');
  assert(ctx.eval('Math.max(1, 5, 3)') === 5, 'Math.max');
  assert(ctx.eval('Math.min(1, 5, 3)') === 1, 'Math.min');
  assert(Math.abs(ctx.eval('Math.sqrt(2)') - Math.SQRT2) < 0.01, 'Math.sqrt');

  // 29. Empty and edge cases
  console.log('\n29. Edge Cases');
  ctx.inject('emptyStr', '');
  assert(ctx.extract('emptyStr') === '', 'Empty string');
  ctx.inject('emptyArr', []);
  var emptyArr = ctx.extract('emptyArr');
  assert(Array.isArray(emptyArr) && emptyArr.length === 0, 'Empty array');
  ctx.inject('emptyObj', {});
  var emptyObj = ctx.extract('emptyObj');
  assert(typeof emptyObj === 'object' && Object.keys(emptyObj).length === 0, 'Empty object');

  assert(ctx.eval('0') === 0, 'Zero');
  assert(ctx.eval('-0') === 0, 'Negative zero'); // Note: -0 === 0 in JS
  assert(ctx.eval("''") === '', 'Empty string literal');

  // 30. Execution timeout (opt-in via enableExecutionTimeLimit)
  // Enforcement uses JSC's private time-limit API resolved through dlsym;
  // these cases require macOS/iOS JavaScriptCore and run via run-tests.sh
  // on macOS only (this whole file is skipped elsewhere).
  console.log('\n30. Execution Timeout (opt-in)');

  // Default (flag off): timeout is accepted but NOT enforced. A busy loop
  // longer than the configured timeout must complete normally. (A true
  // infinite loop would hang the test binary here, by design.)
  var offRt = sandbox.createRuntime({ timeout: 50 });
  var offCtx = offRt.createContext();
  var offResult = offCtx.eval("var t = Date.now(); while (Date.now() - t < 200) {} 'done'");
  assert(offResult === 'done', 'Flag off: loop past the timeout is NOT interrupted');
  offRt.dispose();

  // Opt-in (flag on): infinite loop must be terminated with a clear error
  // and the context must stay usable afterwards.
  var onRt = sandbox.createRuntime({ timeout: 250, enableExecutionTimeLimit: true });
  var onCtx = onRt.createContext();
  var start = Date.now();
  var threw = false;
  var errMsg = '';
  try {
    onCtx.eval('while (true) {}');
  } catch (e) {
    threw = true;
    errMsg = String(e?.message || e);
  }
  var elapsed = Date.now() - start;
  assert(threw, 'Flag on: infinite loop eval throws instead of hanging');
  assert(errMsg.indexOf('timed out') !== -1, 'Timeout error message is explicit', `got: ${errMsg}`);
  assert(elapsed < 5000, 'Interrupt fires promptly after the deadline', `elapsed: ${elapsed}`);
  assert(onCtx.eval('1 + 1') === 2, 'Context stays usable after a timeout');

  // The tenant must not be able to swallow the termination with try/catch.
  var swallowThrew = false;
  try {
    onCtx.eval('try { while (true) {} } catch (e) {} "survived"');
  } catch (_e) {
    swallowThrew = true;
  }
  assert(swallowThrew, 'Tenant try/catch cannot swallow the termination');
  assert(onCtx.eval('6 * 7') === 42, 'Context usable after swallowed-catch attempt');
  onRt.dispose();

  // Flag on + timeout: 0 means unlimited — nothing is armed.
  var unlimitedRt = sandbox.createRuntime({ timeout: 0, enableExecutionTimeLimit: true });
  var unlimitedCtx = unlimitedRt.createContext();
  assert(
    unlimitedCtx.eval('var s = 0; for (var i = 0; i < 100000; i++) s += i; s') === 4999950000,
    'Flag on + timeout 0: unlimited (long loop completes)'
  );
  unlimitedRt.dispose();

  // Flag on + timeout: Infinity must mean "no limit", not a garbage value
  // handed to the native API.
  var infinityRt = sandbox.createRuntime({ timeout: Infinity, enableExecutionTimeLimit: true });
  var infinityCtx = infinityRt.createContext();
  assert(
    infinityCtx.eval('var s = 0; for (var i = 0; i < 100000; i++) s += i; s') === 4999950000,
    'Flag on + timeout Infinity: unlimited (long loop completes)'
  );
  infinityRt.dispose();

  // Promise settlement drains guest reactions (tenant code) during the host
  // microtask drain. It must stay inside the same execution budget.
  var promiseTimeoutRt = sandbox.createRuntime({
    timeout: 250,
    enableExecutionTimeLimit: true,
  });
  var promiseTimeoutCtx = promiseTimeoutRt.createContext();
  promiseTimeoutCtx.inject('timeoutPromise', Promise.resolve(42));
  promiseTimeoutCtx.eval('timeoutPromise.then(function() { while (true) {} });');

  console.log('\nBinary values crossing the sandbox boundary');
  // An ArrayBuffer/TypedArray from the sandbox must arrive host-side as REAL
  // binary (bytes copied), never as the generic property-copy's empty object —
  // that silent destruction was the binary op-batch failure mode. Also
  // exercises the host runtime's createArrayBuffer.
  var binRt = sandbox.createRuntime({});
  var binCtx = binRt.createContext();

  var gotBatch = null;
  binCtx.inject('sendBinary', (buf) => {
    gotBatch = buf;
  });
  binCtx.eval(
    'var ab = new ArrayBuffer(4); var w = new Uint8Array(ab); ' +
      'w[0] = 0x4b; w[1] = 0x45; w[2] = 0x45; w[3] = 0x4c; sendBinary(ab);'
  );
  assert(gotBatch instanceof ArrayBuffer, 'sandbox ArrayBuffer arrives as host ArrayBuffer');
  var gotBytes = gotBatch instanceof ArrayBuffer ? new Uint8Array(gotBatch) : null;
  assert(
    gotBytes &&
      gotBytes.length === 4 &&
      gotBytes[0] === 0x4b &&
      gotBytes[1] === 0x45 &&
      gotBytes[2] === 0x45 &&
      gotBytes[3] === 0x4c,
    'ArrayBuffer bytes survive the boundary intact'
  );

  var gotView = null;
  binCtx.inject('sendView', (v) => {
    gotView = v;
  });
  binCtx.eval('var backing = new Uint8Array([1, 2, 3, 4, 5]); sendView(backing.subarray(1, 4));');
  assert(gotView instanceof Uint8Array, 'sandbox Uint8Array arrives as host Uint8Array');
  assert(
    gotView && gotView.length === 3 && gotView[0] === 2 && gotView[1] === 3 && gotView[2] === 4,
    "only the view's byte window crosses, bytes intact"
  );

  var evalAb = binCtx.eval('new Uint8Array([9, 8, 7]).buffer');
  assert(
    evalAb instanceof ArrayBuffer && new Uint8Array(evalAb)[0] === 9,
    'eval-returned ArrayBuffer crosses intact'
  );

  // A merely view-SHAPED plain object with an out-of-range byteOffset must be
  // rejected by extractHostViewBytes (bounded in the double domain against the
  // real backing size) and fall through to the generic object copy — never a
  // UB size_t cast. 2**64 specifically defeats a naive static_cast<double>
  // (SIZE_MAX) upper bound, which rounds up to 2**64.
  binCtx.inject('getFakeView', () => ({
    constructor: { name: 'Uint8Array' },
    buffer: new ArrayBuffer(4),
    byteOffset: 2 ** 64,
    byteLength: 2,
  }));
  assert(
    binCtx.eval(
      'var v = getFakeView();' +
        // Not real bytes: a plain object copy, NOT a Uint8Array / ArrayBuffer.
        '!(v instanceof Uint8Array) && !(v instanceof ArrayBuffer) && ' +
        'typeof v === "object" && v.byteLength === 2 && v.byteOffset === Math.pow(2,64)'
    ) === true,
    'fake view with byteOffset 2**64 is rejected, copied as a plain object (no UB cast)'
  );
  binRt.dispose();

  // Summary
  console.log('\n=== Test Summary ===');
  console.log(`Total: ${testsRun}`);
  console.log(`Passed: ${testsPassed}`);
  console.log(`Failed: ${testsFailed}`);

  if (testsFailed === 0) {
    console.log('\n✓ ALL TESTS PASSED\n');
  } else {
    console.log('\n✗ SOME TESTS FAILED\n');
  }

  // Return result for the C++ runner
  return testsFailed === 0;
})();
