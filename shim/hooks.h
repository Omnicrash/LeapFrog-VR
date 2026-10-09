// Passthrough: vtable hooks on vrserver's host interfaces, patched in place so drivers loaded
// before LeapFrog (lighthouse) go through them too. Installed only for driver_leapfrog.passthrough.
#pragma once
#include "openvr_driver.h"
#include <stdint.h>

namespace lf_hooks {

void install(vr::IVRDriverContext* context);
void uninstall();

// Set around LeapFrog's own host calls, so the hooks pass them through without asking Rust.
struct OwnCall
{
    OwnCall();
    ~OwnCall();
};

} // namespace lf_hooks
