//! `engine test` harness: `test`/`it`/`describe`/`expect`/`assert` globals and
//! the `bun:test` / `node:test` module surface.
//!
//! The harness is implemented in JavaScript on top of the runtime prelude. That
//! keeps async tests, hooks, and matcher error messages on the same job queue as
//! the program under test; Rust only starts the runner and reads the JSON result
//! array once the promise queue has settled.

use boa_engine::{js_string, Context, JsError, JsResult, Source};

/// One result row produced by the JavaScript runner.
#[derive(Debug, Clone, PartialEq)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub skipped: bool,
    pub error: Option<String>,
    pub duration_ms: f64,
}

impl TestResult {
    pub fn failed(&self) -> bool {
        !self.passed && !self.skipped
    }
}

const HARNESS: &str = r#"
(() => {
    const root = {
        name: "",
        parent: null,
        tests: [],
        suites: [],
        beforeAll: [],
        afterAll: [],
        beforeEach: [],
        afterEach: [],
    };
    let current = root;
    let onlyMode = false;
    const started = Date.now();

    function formatError(error) {
        if (error === null || error === undefined) return String(error);
        if (typeof error === "object") {
            const name = error.name ? String(error.name) : "Error";
            const message = error.message !== undefined ? String(error.message) : String(error);
            return name + ": " + message;
        }
        return String(error);
    }

    function isPlainObject(value) {
        if (value === null || typeof value !== "object") return false;
        const prototype = Object.getPrototypeOf(value);
        return prototype === Object.prototype || prototype === null;
    }

    function deepEqual(a, b) {
        if (Object.is(a, b)) return true;
        if (typeof a !== typeof b) return false;
        if (a === null || b === null) return false;
        if (typeof a !== "object") return false;
        if (a instanceof Date || b instanceof Date) {
            return a instanceof Date && b instanceof Date && a.getTime() === b.getTime();
        }
        if (a instanceof RegExp || b instanceof RegExp) {
            return a instanceof RegExp && b instanceof RegExp && String(a) === String(b);
        }
        if (Array.isArray(a) !== Array.isArray(b)) return false;
        const aKeys = Object.keys(a);
        const bKeys = Object.keys(b);
        if (aKeys.length !== bKeys.length) return false;
        return aKeys.every((key) => {
            return Object.prototype.hasOwnProperty.call(b, key) && deepEqual(a[key], b[key]);
        });
    }

    function fail(message, negated) {
        throw new Error(negated ? "Expected the assertion not to hold: " + message : message);
    }

    function describeValue(value) {
        if (typeof value === "string") return JSON.stringify(value);
        if (typeof value === "bigint") return String(value) + "n";
        if (typeof value === "function") return "[Function " + (value.name || "anonymous") + "]";
        try {
            return JSON.stringify(value);
        } catch (_) {
            return String(value);
        }
    }

    function pathLookup(object, path) {
        const parts = String(path).split(".");
        let cursor = object;
        for (const part of parts) {
            if (cursor === null || cursor === undefined) return { found: false };
            if (!(part in Object(cursor))) return { found: false };
            cursor = cursor[part];
        }
        return { found: true, value: cursor };
    }

    function matchers(actual, negate) {
        const result = {};

        result.toBe = (expected) => {
            if (negate ? Object.is(actual, expected) : !Object.is(actual, expected)) {
                fail("expect(received).toBe(expected)\nreceived: " + describeValue(actual) +
                    "\nexpected: " + describeValue(expected), negate);
            }
        };
        result.toEqual = result.toStrictEqual = (expected) => {
            if (negate ? deepEqual(actual, expected) : !deepEqual(actual, expected)) {
                fail("expect(received).toEqual(expected)\nreceived: " + describeValue(actual) +
                    "\nexpected: " + describeValue(expected), negate);
            }
        };
        const truthy = Boolean(actual);
        result.toBeTruthy = () => {
            if (negate ? truthy : !truthy) fail("expected " + describeValue(actual) + " to be truthy", negate);
        };
        result.toBeFalsy = () => {
            if (negate ? !truthy : truthy) fail("expected " + describeValue(actual) + " to be falsy", negate);
        };
        result.toBeNull = () => {
            if (negate ? actual === null : actual !== null) fail("expected " + describeValue(actual) + " to be null", negate);
        };
        result.toBeUndefined = () => {
            if (negate ? actual === undefined : actual !== undefined) fail("expected " + describeValue(actual) + " to be undefined", negate);
        };
        result.toBeDefined = () => {
            if (negate ? actual !== undefined : actual === undefined) fail("expected value to be defined", negate);
        };
        result.toBeNaN = () => {
            const isNan = typeof actual === "number" && Number.isNaN(actual);
            if (negate ? isNan : !isNan) fail("expected " + describeValue(actual) + " to be NaN", negate);
        };
        result.toBeGreaterThan = (expected) => {
            if (negate ? actual > expected : !(actual > expected)) fail("expected " + describeValue(actual) + " > " + describeValue(expected), negate);
        };
        result.toBeGreaterThanOrEqual = (expected) => {
            if (negate ? actual >= expected : !(actual >= expected)) fail("expected " + describeValue(actual) + " >= " + describeValue(expected), negate);
        };
        result.toBeLessThan = (expected) => {
            if (negate ? actual < expected : !(actual < expected)) fail("expected " + describeValue(actual) + " < " + describeValue(expected), negate);
        };
        result.toBeLessThanOrEqual = (expected) => {
            if (negate ? actual <= expected : !(actual <= expected)) fail("expected " + describeValue(actual) + " <= " + describeValue(expected), negate);
        };
        result.toBeCloseTo = (expected, digits = 2) => {
            const tolerance = Math.pow(10, -digits) / 2;
            const close = Math.abs(actual - expected) <= tolerance;
            if (negate ? close : !close) fail("expected " + describeValue(actual) + " to be close to " + describeValue(expected), negate);
        };
        result.toContain = (expected) => {
            let contains = false;
            if (typeof actual === "string") contains = actual.includes(expected);
            else if (Array.isArray(actual)) contains = actual.some((item) => deepEqual(item, expected));
            else if (actual instanceof Set) contains = actual.has(expected);
            if (negate ? contains : !contains) fail("expected " + describeValue(actual) + " to contain " + describeValue(expected), negate);
        };
        result.toHaveLength = (expected) => {
            const length = actual === null || actual === undefined ? undefined : actual.length;
            if (negate ? length === expected : length !== expected) fail("expected length " + expected + ", received " + describeValue(length), negate);
        };
        result.toMatch = (expected) => {
            const matched = expected instanceof RegExp
                ? expected.test(String(actual))
                : String(actual).includes(String(expected));
            if (negate ? matched : !matched) fail("expected " + describeValue(actual) + " to match " + String(expected), negate);
        };
        result.toHaveProperty = (path, ...expected) => {
            const lookup = pathLookup(actual, path);
            let ok = lookup.found;
            if (ok && expected.length > 0) ok = deepEqual(lookup.value, expected[0]);
            if (negate ? ok : !ok) fail("expected value to have property " + describeValue(path), negate);
        };
        result.toThrow = (expected) => {
            if (typeof actual !== "function") fail("expect(received).toThrow() requires a function", negate);
            let thrown = false;
            let caught;
            try {
                actual();
            } catch (error) {
                thrown = true;
                caught = error;
            }
            let matches = thrown;
            if (matches && expected !== undefined) {
                if (typeof expected === "string") {
                    matches = formatError(caught).includes(expected);
                } else if (expected instanceof RegExp) {
                    matches = expected.test(formatError(caught));
                } else if (typeof expected === "function") {
                    matches = caught instanceof expected;
                } else {
                    matches = deepEqual(caught, expected);
                }
            }
            if (negate ? matches : !matches) {
                fail(thrown ? "expected function not to throw, received " + formatError(caught) : "expected function to throw", negate);
            }
        };

        return result;
    }

    const asyncMatcherNames = [
        "toBe", "toEqual", "toStrictEqual", "toBeTruthy", "toBeFalsy", "toBeNull",
        "toBeUndefined", "toBeDefined", "toBeNaN", "toBeGreaterThan",
        "toBeGreaterThanOrEqual", "toBeLessThan", "toBeLessThanOrEqual",
        "toBeCloseTo", "toContain", "toHaveLength", "toMatch", "toHaveProperty",
        "toThrow",
    ];

    function asyncMatchers(promise, reject) {
        const result = {};
        for (const name of asyncMatcherNames) {
            result[name] = (...expected) => Promise.resolve(promise).then(
                (value) => {
                    if (reject) {
                        throw new Error("Expected promise to reject, but it resolved with " + describeValue(value));
                    }
                    return matchers(value, false)[name](...expected);
                },
                (error) => {
                    if (!reject) throw error;
                    if (name === "toThrow") {
                        const wanted = expected.length > 0 ? expected[0] : undefined;
                        let matched = true;
                        if (wanted !== undefined) {
                            if (typeof wanted === "string") matched = formatError(error).includes(wanted);
                            else if (wanted instanceof RegExp) matched = wanted.test(formatError(error));
                            else if (typeof wanted === "function") matched = error instanceof wanted;
                            else matched = deepEqual(error, wanted);
                        }
                        if (!matched) {
                            throw new Error("Expected rejection to match " + describeValue(wanted) +
                                ", received " + formatError(error));
                        }
                        return;
                    }
                    return matchers(error, false)[name](...expected);
                },
            );
        }
        return result;
    }

    function expect(actual) {
        const api = matchers(actual, false);
        api.not = matchers(actual, true);
        api.resolves = asyncMatchers(actual, false);
        api.rejects = asyncMatchers(actual, true);
        return api;
    }

    function assert(value, message) {
        if (!value) throw new Error(message || "Assertion failed");
    }
    assert.ok = assert;
    assert.equal = (actual, expected, message) => {
        if (actual != expected) throw new Error(message || "Expected " + describeValue(actual) + " to equal " + describeValue(expected));
    };
    assert.notEqual = (actual, expected, message) => {
        if (actual == expected) throw new Error(message || "Expected values to differ");
    };
    assert.strictEqual = (actual, expected, message) => {
        if (!Object.is(actual, expected)) throw new Error(message || "Expected " + describeValue(actual) + " to strictly equal " + describeValue(expected));
    };
    assert.notStrictEqual = (actual, expected, message) => {
        if (Object.is(actual, expected)) throw new Error(message || "Expected values to differ");
    };
    assert.deepEqual = (actual, expected, message) => {
        if (!deepEqual(actual, expected)) throw new Error(message || "Expected values to be deeply equal");
    };
    assert.deepStrictEqual = assert.deepEqual;
    assert.notDeepEqual = (actual, expected, message) => {
        if (deepEqual(actual, expected)) throw new Error(message || "Expected values not to be deeply equal");
    };
    assert.throws = (fn, expected, message) => {
        try {
            expect(fn).toThrow(expected);
        } catch (error) {
            throw new Error(message || formatError(error));
        }
    };
    assert.doesNotThrow = (fn, message) => {
        try {
            fn();
        } catch (error) {
            throw new Error(message || "Expected function not to throw: " + formatError(error));
        }
    };
    assert.rejects = async (promise, expected, message) => {
        try {
            await asyncMatchers(promise, true).toThrow(expected);
        } catch (error) {
            throw new Error(message || formatError(error));
        }
    };
    assert.match = (actual, expected, message) => {
        if (!(expected instanceof RegExp) || !expected.test(String(actual))) {
            throw new Error(message || "Expected string to match " + String(expected));
        }
    };
    assert.fail = (message) => {
        throw new Error(message || "Assertion failed");
    };

    function registerHook(list, fn) {
        if (typeof fn !== "function") throw new TypeError("hook must be a function");
        list.push(fn);
    }

    function normalizeArguments(name, options, fn) {
        if (typeof options === "function") {
            return { name: String(name), options: {}, fn: options };
        }
        return { name: String(name), options: options || {}, fn };
    }

    function test(name, options, fn) {
        const normalized = normalizeArguments(name, options, fn);
        if (typeof normalized.fn !== "function" && !normalized.options.todo) {
            throw new TypeError("test \"" + normalized.name + "\" requires a callback");
        }
        if (normalized.options.only) onlyMode = true;
        current.tests.push({
            name: normalized.name,
            suite: current,
            fn: normalized.fn,
            skip: Boolean(normalized.options.skip || normalized.options.todo || !normalized.fn),
            todo: Boolean(normalized.options.todo),
            only: Boolean(normalized.options.only),
        });
    }
    test.skip = (name, options, fn) => {
        const normalized = normalizeArguments(name, options, fn);
        current.tests.push({ name: normalized.name, suite: current, fn: normalized.fn, skip: true, todo: false, only: false });
    };
    test.todo = (name) => {
        current.tests.push({ name: String(name), suite: current, fn: undefined, skip: true, todo: true, only: false });
    };
    test.only = (name, options, fn) => test(name, Object.assign({}, normalizeArguments(name, options, fn).options, { only: true }), normalizeArguments(name, options, fn).fn);

    function describe(name, options, fn) {
        if (typeof options === "function") {
            fn = options;
            options = {};
        }
        const suite = {
            name: String(name),
            parent: current,
            tests: [],
            suites: [],
            beforeAll: [],
            afterAll: [],
            beforeEach: [],
            afterEach: [],
            skip: Boolean(options && options.skip),
            only: Boolean(options && options.only),
        };
        if (suite.only) onlyMode = true;
        current.suites.push(suite);
        const previous = current;
        current = suite;
        try {
            fn();
        } finally {
            current = previous;
        }
    }
    describe.skip = (name, fn) => describe(name, { skip: true }, fn);
    describe.only = (name, fn) => describe(name, { only: true }, fn);

    function chain(suite, key) {
        const hooks = [];
        for (let cursor = suite; cursor; cursor = cursor.parent) {
            hooks.unshift(...cursor[key]);
        }
        return hooks;
    }

    function suitesOnly(suite) {
        for (let cursor = suite; cursor; cursor = cursor.parent) {
            if (cursor.only) return true;
        }
        return false;
    }

    function selected(test) {
        if (!onlyMode) return true;
        return test.only || suitesOnly(test.suite);
    }

    function fullName(test, prefix) {
        return prefix ? prefix + " > " + test.name : test.name;
    }

    async function runTest(test, prefix, results) {
        const name = fullName(test, prefix);
        if (!selected(test)) return;
        const skipSuite = (() => {
            for (let cursor = test.suite; cursor; cursor = cursor.parent) {
                if (cursor.skip) return true;
            }
            return false;
        })();
        if (test.skip || skipSuite) {
            results.push({ name, passed: true, skipped: true, error: test.todo ? "todo" : null, duration_ms: 0 });
            return;
        }

        const startedAt = Date.now();
        let error = null;
        try {
            for (const hook of chain(test.suite, "beforeEach")) await hook();
            await test.fn();
        } catch (caught) {
            error = formatError(caught);
        }
        try {
            for (const hook of chain(test.suite, "afterEach").reverse()) await hook();
        } catch (caught) {
            if (error === null) error = formatError(caught);
        }
        results.push({ name, passed: error === null, skipped: false, error, duration_ms: Date.now() - startedAt });
    }

    async function runSuite(suite, prefix, results) {
        const name = suite.name ? (prefix ? prefix + " > " + suite.name : suite.name) : prefix;
        let hookError = null;
        try {
            for (const hook of suite.beforeAll) await hook();
        } catch (caught) {
            hookError = formatError(caught);
        }
        for (const test of suite.tests) {
            if (hookError !== null) {
                if (!suite.skip) {
                    results.push({ name: fullName(test, name), passed: false, skipped: false, error: hookError, duration_ms: 0 });
                }
                continue;
            }
            await runTest(test, name, results);
        }
        for (const child of suite.suites) await runSuite(child, name, results);
        if (hookError === null) {
            try {
                for (const hook of suite.afterAll) await hook();
            } catch (caught) {
                results.push({ name: (name || "(root)") + " > afterAll", passed: false, skipped: false, error: formatError(caught), duration_ms: 0 });
            }
        }
    }

    const api = { test, it: test, describe, expect, assert };
    api.beforeEach = (fn) => registerHook(current.beforeEach, fn);
    api.afterEach = (fn) => registerHook(current.afterEach, fn);
    api.beforeAll = (fn) => registerHook(current.beforeAll, fn);
    api.afterAll = (fn) => registerHook(current.afterAll, fn);
    api.before = api.beforeAll;
    api.after = api.afterAll;

    globalThis.__engine_test_api = api;
    globalThis.test = test;
    globalThis.it = test;
    globalThis.describe = describe;
    globalThis.beforeEach = api.beforeEach;
    globalThis.afterEach = api.afterEach;
    globalThis.beforeAll = api.beforeAll;
    globalThis.afterAll = api.afterAll;
    globalThis.expect = expect;
    globalThis.assert = assert;

    globalThis.__engine_test_results = "[]";
    globalThis.__engine_run_tests = async () => {
        const results = [];
        try {
            await runSuite(root, "", results);
        } catch (error) {
            results.push({ name: "(test runner)", passed: false, skipped: false, error: formatError(error), duration_ms: 0 });
        }
        globalThis.__engine_test_results = JSON.stringify(results);
        globalThis.__engine_test_elapsed = Date.now() - started;
        return results.length;
    };
})();
"#;

/// Installs the `test`/`expect` globals and the `bun:test` module surface.
pub(crate) fn install(context: &mut Context) -> JsResult<()> {
    context.eval(Source::from_bytes(HARNESS)).map(|_| ())
}

/// Reads the JSON result array left by the JavaScript runner.
pub(crate) fn extract_results(context: &mut Context) -> JsResult<Vec<TestResult>> {
    let value = context
        .global_object()
        .get(js_string!("__engine_test_results"), context)?;
    let text = value
        .as_string()
        .map(|string| string.to_std_string_escaped())
        .unwrap_or_else(|| "[]".to_owned());
    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        JsError::from(boa_engine::JsNativeError::error().with_message(format!(
            "test runner produced invalid results: {error}"
        )))
    })?;
    let rows = parsed.as_array().cloned().unwrap_or_default();
    Ok(rows
        .into_iter()
        .map(|row| TestResult {
            name: row
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<unnamed>")
                .to_owned(),
            passed: row
                .get("passed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            skipped: row
                .get("skipped")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            error: row
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            duration_ms: row
                .get("duration_ms")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunOptions;

    fn run_harness(source: &str) -> Vec<TestResult> {
        let options = RunOptions {
            filename: "harness-test.js".to_owned(),
            test_mode: true,
            ..RunOptions::default()
        };
        crate::run_tests(source, &options).expect("the harness should run")
    }

    #[test]
    fn passes_and_fails_tests() {
        let results = run_harness(
            r#"
            test("passes", () => { expect(1 + 1).toBe(2); });
            test("fails", () => { expect(1 + 1).toBe(3); });
            "#,
        );
        assert_eq!(results.len(), 2);
        assert!(results[0].passed);
        assert!(!results[1].passed);
        assert!(results[1].error.as_deref().unwrap_or("").contains("toBe"));
    }

    #[test]
    fn supports_async_tests_and_hooks() {
        let results = run_harness(
            r#"
            let order = [];
            beforeEach(() => { order.push("before"); });
            afterEach(() => { order.push("after"); });
            test("async", async () => {
                await Promise.resolve();
                expect(order).toEqual(["before"]);
            });
            test("order", () => {
                expect(order).toEqual(["before", "after", "before"]);
            });
            "#,
        );
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(TestResult::failed_or_passed), "{results:?}");
    }

    #[test]
    fn supports_describe_and_skip() {
        let results = run_harness(
            r#"
            describe("math", () => {
                test("adds", () => expect(2 + 2).toBe(4));
                test.skip("skips", () => { throw new Error("never"); });
            });
            "#,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "math > adds");
        assert!(results[1].skipped);
    }

    #[test]
    fn supports_assert_and_rejections() {
        let results = run_harness(
            r#"
            test("assert", () => { assert.strictEqual(1, 1); });
            test("rejects", async () => {
                await expect(Promise.reject(new Error("boom"))).rejects.toThrow("boom");
            });
            test("resolves", async () => {
                await expect(Promise.resolve(41)).resolves.toBe(41);
            });
            "#,
        );
        assert!(results.iter().all(|result| result.passed), "{results:?}");
    }
}

#[cfg(test)]
trait TestResultExt {
    fn failed_or_passed(&self) -> bool;
}

#[cfg(test)]
impl TestResultExt for TestResult {
    fn failed_or_passed(&self) -> bool {
        self.passed && self.error.is_none()
    }
}
