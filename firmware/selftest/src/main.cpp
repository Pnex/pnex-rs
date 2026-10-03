//
// PneX pin self-test bench firmware (D122) — offline serial console, see
// lib/pnex/src/pnex_selftest.h. Never shipped to a registered device: the
// server builder only builds the generic_* projects.
//

#include <Pnex.h>
#include <pnex_selftest.h>

void setup() {
    pnex_selftest_run();
}

void loop() {}
