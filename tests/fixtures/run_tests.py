import os, sys, time
c = os.environ.get("FAKE_COUNTER")
if c:
    with open(c, "a") as f:
        f.write("x\n")
time.sleep(float(os.environ.get("FAKE_SLEEP", "0")))
if "zero" in sys.argv:
    print("running 0 tests")
    print("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.00s")
    sys.exit(0)
if any("::" in a for a in sys.argv[1:]):
    # A name filter: one test runs, the rest are filtered out (libtest reports that).
    print("running 1 test")
    print("test math::adds ... ok")
    print("")
    print("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s")
    sys.exit(0)
fail = os.environ.get("FAKE_FAIL") == "1"
print("running 3 tests")
print("test math::adds ... ok")
print("test math::subs ... ok")
print("test math::divs ... " + ("FAILED" if fail else "ok"))
print("")
print("test result: %s. %d passed; %d failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s" % ("FAILED" if fail else "ok", 2 if fail else 3, 1 if fail else 0))
sys.exit(101 if fail else 0)
