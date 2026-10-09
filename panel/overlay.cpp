// The dashboard tab: a D3D11 render target drawn by Dear ImGui, handed to SteamVR as the
// overlay's texture, with the laser pointer's events fed back to ImGui as a mouse. No window.
#include "panel.h"

#include <windows.h>
#include <d3d11.h>
#include <openvr.h>

#include "imgui.h"
#include "backends/imgui_impl_dx11.h"

#include <cmath>
#include <cstdio>
#include <cstring>

namespace {

const int WIDTH = 1200;
const int HEIGHT = 800;
const float WIDTH_METERS = 2.2f;
const char* const KEY = "omnicrash.leapfrog";

vr::IVROverlay* g_overlay;
vr::IVRSystem* g_system;
vr::VROverlayHandle_t g_main = vr::k_ulOverlayHandleInvalid;
vr::VROverlayHandle_t g_thumb = vr::k_ulOverlayHandleInvalid;

ID3D11Device* g_device;
ID3D11DeviceContext* g_context;
ID3D11Texture2D* g_texture;
ID3D11RenderTargetView* g_target;

bool g_visible;

}  // namespace

void read_all();

extern "C" int lfp_overlay_open(const char* thumbnail, char* err, size_t err_len) {
    g_overlay = static_cast<vr::IVROverlay*>(lfp_interface(vr::IVROverlay_Version, err, err_len));
    g_system = static_cast<vr::IVRSystem*>(lfp_interface(vr::IVRSystem_Version, err, err_len));
    if (!g_overlay || !g_system) {
        return 1;
    }
    vr::EVROverlayError oe = g_overlay->CreateDashboardOverlay(KEY, "LeapFrog", &g_main, &g_thumb);
    if (oe != vr::VROverlayError_None) {
        snprintf(err, err_len, "dashboard overlay: %s", g_overlay->GetOverlayErrorNameFromEnum(oe));
        return 1;
    }
    g_overlay->SetOverlayWidthInMeters(g_main, WIDTH_METERS);
    g_overlay->SetOverlayInputMethod(g_main, vr::VROverlayInputMethod_Mouse);
    vr::HmdVector2_t scale = {{static_cast<float>(WIDTH), static_cast<float>(HEIGHT)}};
    g_overlay->SetOverlayMouseScale(g_main, &scale);
    g_overlay->SetOverlayFlag(g_main, vr::VROverlayFlags_SendVRDiscreteScrollEvents, true);
    if (thumbnail && *thumbnail) {
        g_overlay->SetOverlayFromFile(g_thumb, thumbnail);
    }

    D3D_FEATURE_LEVEL level;
    HRESULT hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION, &g_device, &level, &g_context);
    if (FAILED(hr)) {
        snprintf(err, err_len, "D3D11CreateDevice failed (0x%08lx)", static_cast<unsigned long>(hr));
        return 1;
    }
    D3D11_TEXTURE2D_DESC desc = {};
    desc.Width = WIDTH;
    desc.Height = HEIGHT;
    desc.MipLevels = 1;
    desc.ArraySize = 1;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.SampleDesc.Count = 1;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
    hr = g_device->CreateTexture2D(&desc, nullptr, &g_texture);
    if (SUCCEEDED(hr)) {
        hr = g_device->CreateRenderTargetView(g_texture, nullptr, &g_target);
    }
    if (FAILED(hr)) {
        snprintf(err, err_len, "render target failed (0x%08lx)", static_cast<unsigned long>(hr));
        return 1;
    }

    IMGUI_CHECKVERSION();
    ImGui::CreateContext();
    ImGuiIO& io = ImGui::GetIO();
    io.IniFilename = nullptr;
    io.DisplaySize = ImVec2(static_cast<float>(WIDTH), static_cast<float>(HEIGHT));
    // Segoe UI at a size that reads in the headset; ImGui's built-in font as the fallback.
    if (!io.Fonts->AddFontFromFileTTF("C:\\Windows\\Fonts\\segoeui.ttf", 30.0f)) {
        ImFontConfig cfg;
        cfg.SizePixels = 26.0f;
        io.Fonts->AddFontDefault(&cfg);
    }
    ImGui::StyleColorsDark();
    ImGuiStyle& style = ImGui::GetStyle();
    style.ScaleAllSizes(2.0f);
    style.WindowRounding = 0.0f;
    style.FramePadding = ImVec2(18.0f, 14.0f);
    style.ItemSpacing = ImVec2(16.0f, 14.0f);
    style.GrabMinSize = 40.0f;
    ImGui_ImplDX11_Init(g_device, g_context);
    read_all();
    return 0;
}

// The settings the panel edits. Floats get a stepper row (minus, slider, plus); bools a toggle.
// `live` ones the driver reads once a second; the others take effect at the next SteamVR start.
struct FloatSetting {
    const char* label;
    const char* key;
    float min, max, step, def;
    const char* format;
    float value;
    const char* note; // gray explanation under the row, or null
};

struct BoolSetting {
    const char* label;
    const char* key;
    bool def;
    bool live;
    bool value;
    const char* note; // gray explanation after the label, or null
    const char* warning; // amber line under the note, within the row, or null
};

FloatSetting g_placement[] = {
    {"Forward / back", "handOffsetZ", -0.25f, 0.25f, 0.01f, -0.08f, "%.2f m", 0, nullptr},
    {"Up / down", "handOffsetY", -0.25f, 0.25f, 0.01f, 0.06f, "%.2f m", 0, nullptr},
    {"Left / right", "handOffsetX", -0.25f, 0.25f, 0.01f, -0.01f, "%.2f m", 0, nullptr},
    {"Inward", "handInwardOffset", -0.10f, 0.10f, 0.005f, 0.01f, "%.3f m", 0, nullptr},
    {"Pitch", "rotationOffsetX", -45.0f, 45.0f, 1.0f, 0.0f, "%.0f deg", 0, nullptr},
    {"Yaw", "rotationOffsetY", -45.0f, 45.0f, 1.0f, 0.0f, "%.0f deg", 0, nullptr},
    {"Roll", "rotationOffsetZ", -45.0f, 45.0f, 1.0f, 0.0f, "%.0f deg", 0, nullptr},
};

FloatSetting g_switching[] = {
    {"Away distance", "awayDistance", 0.05f, 0.60f, 0.01f, 0.20f, "%.2f m", 0, "When your hand is this far from a resting controller, hand tracking takes over"},
    {"Near distance", "nearDistance", 0.05f, 0.40f, 0.01f, 0.15f, "%.2f m", 0, "When your hand comes this close to the controller, it takes over"},
    {"Still speed", "stillSpeed", 0.0f, 1.0f, 0.05f, 0.30f, "%.2f m/s", 0, "A controller moving slower than this counts as resting"},
    {"Pickup speed", "pickupSpeed", 0.0f, 1.5f, 0.05f, 0.40f, "%.2f m/s", 0, "A controller moving faster than this counts as picked up"},
};

BoolSetting g_pickup = {"Automatic switching", "autoPickupSwitch", true, true, true, "If disabled, hands only track while controllers are off", nullptr};

BoolSetting g_driver[] = {
    {"Passthrough", "passthrough", true, false, true, "The controller shows the tracked hand while resting", nullptr},
    {"Full tracking mode", "exposeAsFullTrackingMode", false, false, false, "Tell games the controller is fully hand tracked", "Breaks the pointer and the B menu button in VRChat"},
    {"Manage service", "manageService", true, false, true, "Automatically start and stop the Ultraleap service with SteamVR", nullptr},
    {"Hand on object", "handOnObject", true, false, true, "Hint to the Leap that hands might be holding objects", nullptr},
    {"Fast hand motion", "fastHandMotion", false, false, false, "Favor quick movement over precision", nullptr},
};

const char* const PERFORMANCE[] = {"low", "default", "ultra"};
int g_performance;

float read_float(const FloatSetting& s) {
    float f;
    if (lfp_get_float(s.key, &f) == 0) {
        return f;
    }
    int i;
    return lfp_get_int(s.key, &i) == 0 ? static_cast<float>(i) : s.def;
}

bool read_bool(const BoolSetting& s) {
    int b;
    return lfp_get_bool(s.key, &b) == 0 ? b != 0 : s.def;
}

void read_all() {
    for (FloatSetting& s : g_placement) s.value = read_float(s);
    for (FloatSetting& s : g_switching) s.value = read_float(s);
    g_pickup.value = read_bool(g_pickup);
    for (BoolSetting& s : g_driver) s.value = read_bool(s);
    char buf[32] = {};
    lfp_get_string("trackingPerformance", buf, sizeof buf);
    g_performance = 0;
    for (int i = 0; i < 3; i++) {
        if (strcmp(buf, PERFORMANCE[i]) == 0) g_performance = i;
    }
}

void write(FloatSetting& s, float v) {
    v = v < s.min ? s.min : v > s.max ? s.max : v;
    // Steps land on round values, not on float dust.
    v = roundf(v / s.step) * s.step;
    if (v != s.value) {
        s.value = v;
        lfp_set_float(s.key, v);
    }
}

void write(BoolSetting& s, bool v) {
    if (v != s.value) {
        s.value = v;
        lfp_set_bool(s.key, v ? 1 : 0);
    }
}

// One row: label, a large minus, the slider with the value on it, a large plus.
void stepper(FloatSetting& s) {
    const float button = 90.0f;
    ImGui::PushID(s.key);
    ImGui::AlignTextToFramePadding();
    ImGui::TextUnformatted(s.label);
    ImGui::SameLine(330.0f);
    if (ImGui::Button("-", ImVec2(button, 0))) write(s, s.value - s.step);
    ImGui::SameLine();
    ImGui::SetNextItemWidth(ImGui::GetContentRegionAvail().x - button - ImGui::GetStyle().ItemSpacing.x);
    float v = s.value;
    if (ImGui::SliderFloat("##slider", &v, s.min, s.max, s.format, ImGuiSliderFlags_AlwaysClamp)) write(s, v);
    ImGui::SameLine();
    if (ImGui::Button("+", ImVec2(button, 0))) write(s, s.value + s.step);
    if (s.note) {
        ImGui::Indent(330.0f);
        ImGui::TextDisabled("%s", s.note);
        ImGui::Unindent(330.0f);
    }
    ImGui::PopID();
}

// A toggle: a wide button that reads On or Off, green while on. The note (about 50 characters
// at most, it shares the line with the button) goes after the label.
void toggle(BoolSetting& s) {
    ImGui::PushID(s.key);
    ImGui::AlignTextToFramePadding();
    ImGui::TextUnformatted(s.label);
    if (s.note && s.warning) {
        // Two lines in the row's height: the note on top, the amber warning right under it.
        const ImVec2 row = ImGui::GetCursorPos();
        const float top = row.y - ImGui::GetStyle().ItemSpacing.y - ImGui::GetFrameHeight();
        ImGui::SetCursorPos(ImVec2(330.0f, top - 1.0f));
        ImGui::TextDisabled("%s", s.note);
        ImGui::SetCursorPos(ImVec2(330.0f, top + ImGui::GetTextLineHeight() - 1.0f));
        ImGui::TextColored(ImVec4(0.95f, 0.70f, 0.25f, 1.0f), "%s", s.warning);
        ImGui::SetCursorPos(ImVec2(ImGui::GetContentRegionMax().x - 160.0f, top));
    } else {
        if (s.note) {
            ImGui::SameLine(330.0f);
            ImGui::TextDisabled("%s", s.note);
        }
        ImGui::SameLine(ImGui::GetContentRegionMax().x - 160.0f);
    }
    if (s.value) ImGui::PushStyleColor(ImGuiCol_Button, ImVec4(0.18f, 0.50f, 0.25f, 1.0f));
    if (ImGui::Button(s.value ? "On" : "Off", ImVec2(160, 0))) write(s, !s.value);
    if (s.value) ImGui::PopStyleColor();
    ImGui::PopID();
}

// The Defaults button at the bottom edge of the panel.
void defaults_button(FloatSetting* settings, size_t count) {
    ImGui::SetCursorPosY(ImGui::GetWindowHeight() - ImGui::GetStyle().WindowPadding.y - ImGui::GetFrameHeight());
    if (ImGui::Button("Defaults", ImVec2(220, 0))) {
        for (size_t i = 0; i < count; i++) write(settings[i], settings[i].def);
    }
}

// Draws one frame of the panel: three tabs, placement, switching and driver.
static void draw() {
    ImGui::SetNextWindowPos(ImVec2(0, 0));
    ImGui::SetNextWindowSize(ImGui::GetIO().DisplaySize);
    ImGui::Begin("LeapFrog", nullptr, ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoResize | ImGuiWindowFlags_NoSavedSettings);
    ImGui::Text("LeapFrog %s", LFP_VERSION);
    if (ImGui::BeginTabBar("tabs")) {
        if (ImGui::BeginTabItem("  Hand placement  ")) {
            ImGui::Dummy(ImVec2(0, 10));
            for (FloatSetting& s : g_placement) stepper(s);
            defaults_button(g_placement, sizeof g_placement / sizeof g_placement[0]);
            ImGui::EndTabItem();
        }
        if (ImGui::BeginTabItem("  Switching  ")) {
            ImGui::Dummy(ImVec2(0, 10));
            toggle(g_pickup);
            ImGui::Dummy(ImVec2(0, 10));
            for (FloatSetting& s : g_switching) stepper(s);
            defaults_button(g_switching, sizeof g_switching / sizeof g_switching[0]);
            ImGui::EndTabItem();
        }
        if (ImGui::BeginTabItem("  Driver  ")) {
            ImGui::PushStyleVar(ImGuiStyleVar_ItemSpacing, ImVec2(16.0f, 8.0f));
            for (BoolSetting& s : g_driver) toggle(s);
            ImGui::AlignTextToFramePadding();
            ImGui::TextUnformatted("Performance");
            ImGui::SameLine(330.0f);
            ImGui::TextDisabled("Ultraleap tracking performance mode");
            ImGui::SameLine(ImGui::GetContentRegionMax().x - 3 * 130.0f - 2 * ImGui::GetStyle().ItemSpacing.x);
            for (int i = 0; i < 3; i++) {
                if (i) ImGui::SameLine();
                bool on = g_performance == i;
                if (on) ImGui::PushStyleColor(ImGuiCol_Button, ImVec4(0.18f, 0.50f, 0.25f, 1.0f));
                if (ImGui::Button(PERFORMANCE[i], ImVec2(130, 0)) && !on) {
                    g_performance = i;
                    lfp_set_string("trackingPerformance", PERFORMANCE[i]);
                }
                if (on) ImGui::PopStyleColor();
            }
            ImGui::PopStyleVar();
            // The restart note at the bottom edge of the panel.
            ImGui::SetCursorPosY(ImGui::GetWindowHeight() - ImGui::GetStyle().WindowPadding.y - ImGui::GetTextLineHeight());
            ImGui::TextColored(ImVec4(0.95f, 0.70f, 0.25f, 1.0f), "SteamVR will need to be restarted for these settings to take effect.");
            ImGui::EndTabItem();
        }
        ImGui::EndTabBar();
    }
    ImGui::End();
}

// Pumps the overlay's events into ImGui and renders when the tab is visible. Returns 1 when
// SteamVR asked the process to quit.
extern "C" int lfp_overlay_frame(void) {
    ImGuiIO& io = ImGui::GetIO();
    vr::VREvent_t ev;
    while (g_overlay->PollNextOverlayEvent(g_main, &ev, sizeof(ev))) {
        switch (ev.eventType) {
            case vr::VREvent_MouseMove:
                // The event's y runs up from the bottom; ImGui's runs down from the top.
                io.AddMousePosEvent(ev.data.mouse.x, HEIGHT - ev.data.mouse.y);
                break;
            case vr::VREvent_MouseButtonDown:
            case vr::VREvent_MouseButtonUp:
                io.AddMousePosEvent(ev.data.mouse.x, HEIGHT - ev.data.mouse.y);
                io.AddMouseButtonEvent(ev.data.mouse.button == vr::VRMouseButton_Right ? 1 : 0, ev.eventType == vr::VREvent_MouseButtonDown);
                break;
            case vr::VREvent_ScrollDiscrete:
                io.AddMouseWheelEvent(ev.data.scroll.xdelta, ev.data.scroll.ydelta);
                break;
            case vr::VREvent_FocusLeave:
                io.AddFocusEvent(false);
                break;
            case vr::VREvent_OverlayShown:
                g_visible = true;
                read_all(); // tune.txt or a terminal may have changed them meanwhile
                break;
            case vr::VREvent_OverlayHidden:
                g_visible = false;
                break;
            case vr::VREvent_Quit:
                g_system->AcknowledgeQuit_Exiting();
                return 1;
            default:
                break;
        }
    }
    g_visible = g_overlay->IsOverlayVisible(g_main);
    if (!g_visible) {
        return 0;
    }
    io.DeltaTime = 1.0f / 60.0f;
    ImGui_ImplDX11_NewFrame();
    ImGui::NewFrame();
    draw();
    ImGui::Render();
    const float clear[4] = {0.09f, 0.09f, 0.11f, 1.0f};
    g_context->OMSetRenderTargets(1, &g_target, nullptr);
    g_context->ClearRenderTargetView(g_target, clear);
    ImGui_ImplDX11_RenderDrawData(ImGui::GetDrawData());
    g_context->Flush();
    vr::Texture_t tex = {g_texture, vr::TextureType_DirectX, vr::ColorSpace_Auto};
    g_overlay->SetOverlayTexture(g_main, &tex);
    return 0;
}

extern "C" int lfp_overlay_visible(void) {
    return g_visible ? 1 : 0;
}

extern "C" void lfp_overlay_close(void) {
    if (g_device) {
        ImGui_ImplDX11_Shutdown();
        ImGui::DestroyContext();
    }
    if (g_overlay && g_main != vr::k_ulOverlayHandleInvalid) {
        g_overlay->DestroyOverlay(g_main);
    }
    if (g_target) g_target->Release();
    if (g_texture) g_texture->Release();
    if (g_context) g_context->Release();
    if (g_device) g_device->Release();
    g_target = nullptr;
    g_texture = nullptr;
    g_context = nullptr;
    g_device = nullptr;
}
