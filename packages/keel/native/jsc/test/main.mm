/*
 * JSC Sandbox Test Runner
 *
 * This minimal C++ runner:
 * 1. Creates a JSI Runtime using Meta's official JSCRuntime
 * 2. Installs the sandbox module
 * 3. Runs the JavaScript test suite
 *
 * The actual tests are in sandbox_test.js - testing JS from JS,
 * which reflects the real usage scenario.
 */

#include "../src/JSCRuntime.h"
#include "../src/JSCSandboxJSI.h"
#import <Foundation/Foundation.h>
#include <fstream>
#include <iostream>
#include <sstream>

using namespace facebook;

std::string readFile(const std::string &path) {
  std::ifstream file(path);
  if (!file.is_open()) {
    throw std::runtime_error("Failed to open file: " + path);
  }
  std::stringstream buffer;
  buffer << file.rdbuf();
  return buffer.str();
}

// Install a minimal console.log for the host runtime
void installConsole(jsi::Runtime &runtime) {
  auto console = jsi::Object(runtime);

  auto log = jsi::Function::createFromHostFunction(
      runtime, jsi::PropNameID::forAscii(runtime, "log"), 1,
      [](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args,
         size_t count) -> jsi::Value {
        (void)thisVal;
        for (size_t i = 0; i < count; i++) {
          if (i > 0)
            std::cout << " ";
          if (args[i].isString()) {
            std::cout << args[i].asString(rt).utf8(rt);
          } else if (args[i].isNumber()) {
            std::cout << args[i].asNumber();
          } else if (args[i].isBool()) {
            std::cout << (args[i].getBool() ? "true" : "false");
          } else if (args[i].isNull()) {
            std::cout << "null";
          } else if (args[i].isUndefined()) {
            std::cout << "undefined";
          } else {
            std::cout << args[i].toString(rt).utf8(rt);
          }
        }
        std::cout << std::endl;
        return jsi::Value::undefined();
      });

  console.setProperty(runtime, "log", std::move(log));

  // Alias warn/error to log
  console.setProperty(runtime, "warn", console.getProperty(runtime, "log"));
  console.setProperty(runtime, "error", console.getProperty(runtime, "log"));

  runtime.global().setProperty(runtime, "console", std::move(console));
}

int main(int argc, const char *argv[]) {
  (void)argc;
  (void)argv;
  @autoreleasepool {
    std::cout << "==========================================" << std::endl;
    std::cout << "JSC Sandbox Test Runner" << std::endl;
    std::cout << "(Using Meta's official JSCRuntime)" << std::endl;
    std::cout << "==========================================" << std::endl;

    try {
      // 1. Create the host JSI runtime using Meta's factory
      auto runtime = jsc::makeJSCRuntime();

      // 2. Install console for output
      installConsole(*runtime);

      // 3. Install the sandbox module onto the runtime
      jsc_sandbox::JSCSandboxModule::install(*runtime);

      // 4. Find and load the JavaScript test file
      std::vector<std::string> searchPaths = {
          "test/sandbox_test.js", "./test/sandbox_test.js",
          "../test/sandbox_test.js", "sandbox_test.js"};

      std::string testCode;
      bool found = false;

      for (const auto &path : searchPaths) {
        try {
          testCode = readFile(path);
          found = true;
          std::cout << "Loaded test file: " << path << std::endl;
          break;
        } catch (...) {
          // Try next path
        }
      }

      if (!found) {
        std::cerr << "Error: Could not find sandbox_test.js" << std::endl;
        return 1;
      }

      // 5. Run the JavaScript tests
      auto buffer = std::make_shared<jsi::StringBuffer>(testCode);
      jsi::Value result =
          runtime->evaluateJavaScript(buffer, "sandbox_test.js");

      // 6. Run deferred assertions. Returning from the evaluation above hands
      // control back to the runtime, which drains the host Promise microtasks
      // that the single synchronous test script cannot observe.
      auto phase2Buffer = std::make_shared<jsi::StringBuffer>(
          "typeof __jscSandboxTestPhase2 === 'function'"
          " ? __jscSandboxTestPhase2() : true");
      jsi::Value phase2Result =
          runtime->evaluateJavaScript(phase2Buffer, "sandbox_test_phase2.js");

      // 7. Check results (the JS test returns true/false)
      bool passed = result.isBool() && result.getBool() &&
                    phase2Result.isBool() && phase2Result.getBool();
      return passed ? 0 : 1;

    } catch (const jsi::JSError &e) {
      std::cerr << "JavaScript Error: " << e.getMessage() << std::endl;
      std::cerr << "Stack: " << e.getStack() << std::endl;
      return 1;
    } catch (const std::exception &e) {
      std::cerr << "Error: " << e.what() << std::endl;
      return 1;
    }
  }

  return 0;
}
