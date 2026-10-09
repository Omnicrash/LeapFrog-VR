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

// Frames rendered after the last input, so ImGui's hover and active states settle.
const int SETTLE_FRAMES = 10;
const float LABEL_COLUMN = 330.0f;
const ImVec4 GREEN(0.18f, 0.50f, 0.25f, 1.0f);
const ImVec4 AMBER(0.95f, 0.70f, 0.25f, 1.0f);

vr::IVROverlay* g_overlay;
vr::IVRSystem* g_system;
vr::VROverlayHandle_t g_main = vr::k_ulOverlayHandleInvalid;
vr::VROverlayHandle_t g_thumb = vr::k_ulOverlayHandleInvalid;

ID3D11Device* g_device;
ID3D11DeviceContext* g_context;
ID3D11Texture2D* g_texture;
ID3D11RenderTargetView* g_target;
bool g_imgui;

bool g_visible;
int g_pending; // frames still to render
ULONGLONG g_checked; // last visibility check while hidden

// The settings the panel edits. Floats get a stepper row (minus, slider, plus); bools a toggle.
// Each is read when the tab is shown and written the moment it changes.
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

BoolSetting g_pickup = {"Automatic switching", "autoPickupSwitch", true, true, "If disabled, hands only track while controllers are off", nullptr};

BoolSetting g_driver[] = {
    {"Passthrough", "passthrough", true, true, "The controller shows the tracked hand while resting", nullptr},
    {"Full tracking mode", "exposeAsFullTrackingMode", false, false, "Tell games the controller is fully hand tracked", "Breaks the pointer and the B menu button in VRChat"},
    {"Full hand tracking when standalone (controllers off)", "handDeviceFullTrackingMode", true, true, nullptr, nullptr},
    {"Manage service", "manageService", true, true, "Automatically start and stop the Ultraleap service with SteamVR", nullptr},
    {"Hand on object", "handOnObject", true, true, "Hint to the Leap that hands might be holding objects", nullptr},
    {"Fast hand motion", "fastHandMotion", false, false, "Favor quick movement over precision", nullptr},
};

const char* const PERFORMANCE[] = {"low", "default", "ultra"};
const int PERFORMANCE_DEFAULT = 2; // the driver's default, ultra
int g_performance = PERFORMANCE_DEFAULT;

float read_float(const FloatSetting& s) {
    float f;
    return lfp_get_float(s.key, &f) == 0 ? f : s.def;
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
    g_performance = PERFORMANCE_DEFAULT;
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

// A button drawn green while its option is the current one.
bool highlighted_button(const char* label, bool on, ImVec2 size) {
    if (on) ImGui::PushStyleColor(ImGuiCol_Button, GREEN);
    bool clicked = ImGui::Button(label, size);
    if (on) ImGui::PopStyleColor();
    return clicked;
}

// One row: label, a large minus, the slider with the value on it, a large plus.
void stepper(FloatSetting& s) {
    const float button = 90.0f;
    ImGui::PushID(s.key);
    ImGui::AlignTextToFramePadding();
    ImGui::TextUnformatted(s.label);
    ImGui::SameLine(LABEL_COLUMN);
    if (ImGui::Button("-", ImVec2(button, 0))) write(s, s.value - s.step);
    ImGui::SameLine();
    ImGui::SetNextItemWidth(ImGui::GetContentRegionAvail().x - button - ImGui::GetStyle().ItemSpacing.x);
    float v = s.value;
    if (ImGui::SliderFloat("##slider", &v, s.min, s.max, s.format, ImGuiSliderFlags_AlwaysClamp)) write(s, v);
    ImGui::SameLine();
    if (ImGui::Button("+", ImVec2(button, 0))) write(s, s.value + s.step);
    if (s.note) {
        ImGui::Indent(LABEL_COLUMN);
        ImGui::TextDisabled("%s", s.note);
        ImGui::Unindent(LABEL_COLUMN);
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
        ImGui::SetCursorPos(ImVec2(LABEL_COLUMN, top - 1.0f));
        ImGui::TextDisabled("%s", s.note);
        ImGui::SetCursorPos(ImVec2(LABEL_COLUMN, top + ImGui::GetTextLineHeight() - 1.0f));
        ImGui::TextColored(AMBER, "%s", s.warning);
        ImGui::SetCursorPos(ImVec2(ImGui::GetContentRegionMax().x - 160.0f, top));
    } else {
        if (s.note) {
            ImGui::SameLine(LABEL_COLUMN);
            ImGui::TextDisabled("%s", s.note);
        }
        ImGui::SameLine(ImGui::GetContentRegionMax().x - 160.0f);
    }
    if (highlighted_button(s.value ? "On" : "Off", s.value, ImVec2(160, 0))) write(s, !s.value);
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
void draw() {
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
            ImGui::SameLine(LABEL_COLUMN);
            ImGui::TextDisabled("Ultraleap tracking performance mode");
            ImGui::SameLine(ImGui::GetContentRegionMax().x - 3 * 130.0f - 2 * ImGui::GetStyle().ItemSpacing.x);
            for (int i = 0; i < 3; i++) {
                if (i) ImGui::SameLine();
                bool on = g_performance == i;
                if (highlighted_button(PERFORMANCE[i], on, ImVec2(130, 0)) && !on) {
                    g_performance = i;
                    lfp_set_string("trackingPerformance", PERFORMANCE[i]);
                }
            }
            ImGui::PopStyleVar();
            // The restart note at the bottom edge of the panel.
            ImGui::SetCursorPosY(ImGui::GetWindowHeight() - ImGui::GetStyle().WindowPadding.y - ImGui::GetTextLineHeight());
            ImGui::TextColored(AMBER, "SteamVR will need to be restarted for these settings to take effect.");
            ImGui::EndTabItem();
        }
        ImGui::EndTabBar();
    }
    ImGui::End();
}

bool create_overlay(const char* thumbnail, char* err, size_t err_len) {
    g_overlay = static_cast<vr::IVROverlay*>(lfp_interface(vr::IVROverlay_Version, err, err_len));
    g_system = static_cast<vr::IVRSystem*>(lfp_interface(vr::IVRSystem_Version, err, err_len));
    if (!g_overlay || !g_system) {
        return false;
    }
    vr::EVROverlayError oe = g_overlay->CreateDashboardOverlay(KEY, "LeapFrog", &g_main, &g_thumb);
    if (oe != vr::VROverlayError_None) {
        snprintf(err, err_len, "dashboard overlay: %s", g_overlay->GetOverlayErrorNameFromEnum(oe));
        return false;
    }
    g_overlay->SetOverlayWidthInMeters(g_main, WIDTH_METERS);
    g_overlay->SetOverlayInputMethod(g_main, vr::VROverlayInputMethod_Mouse);
    vr::HmdVector2_t scale = {{static_cast<float>(WIDTH), static_cast<float>(HEIGHT)}};
    g_overlay->SetOverlayMouseScale(g_main, &scale);
    g_overlay->SetOverlayFlag(g_main, vr::VROverlayFlags_SendVRDiscreteScrollEvents, true);
    if (thumbnail && *thumbnail) {
        g_overlay->SetOverlayFromFile(g_thumb, thumbnail);
    }
    return true;
}

bool create_device(char* err, size_t err_len) {
    D3D_FEATURE_LEVEL level;
    HRESULT hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION, &g_device, &level, &g_context);
    if (FAILED(hr)) {
        snprintf(err, err_len, "D3D11CreateDevice failed (0x%08lx)", static_cast<unsigned long>(hr));
        return false;
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
        return false;
    }
    return true;
}

void init_imgui() {
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
    g_imgui = true;
}

void render() {
    ImGui::GetIO().DeltaTime = 1.0f / 60.0f;
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
}

// The tab came up: fresh values (`leapfrog-panel set` from a terminal may have changed them
// meanwhile) and a render.
void shown() {
    g_visible = true;
    read_all();
    g_pending = SETTLE_FRAMES;
}

}  // namespace

extern "C" int lfp_overlay_open(const char* thumbnail, char* err, size_t err_len) {
    if (!create_overlay(thumbnail, err, err_len) || !create_device(err, err_len)) {
        return 1;
    }
    init_imgui();
    read_all();
    return 0;
}

// Pumps the overlay's events into ImGui and renders while input keeps arriving, plus a few
// frames after it; an idle tab costs no rendering. Returns 1 when SteamVR asked the process to
// quit.
extern "C" int lfp_overlay_frame(void) {
    ImGuiIO& io = ImGui::GetIO();
    vr::VREvent_t ev;
    while (g_overlay->PollNextOverlayEvent(g_main, &ev, sizeof(ev))) {
        switch (ev.eventType) {
            case vr::VREvent_MouseMove:
                // The event's y runs up from the bottom; ImGui's runs down from the top.
                io.AddMousePosEvent(ev.data.mouse.x, HEIGHT - ev.data.mouse.y);
                g_pending = SETTLE_FRAMES;
                break;
            case vr::VREvent_MouseButtonDown:
            case vr::VREvent_MouseButtonUp:
                io.AddMousePosEvent(ev.data.mouse.x, HEIGHT - ev.data.mouse.y);
                io.AddMouseButtonEvent(ev.data.mouse.button == vr::VRMouseButton_Right ? 1 : 0, ev.eventType == vr::VREvent_MouseButtonDown);
                g_pending = SETTLE_FRAMES;
                break;
            case vr::VREvent_ScrollDiscrete:
                io.AddMouseWheelEvent(ev.data.scroll.xdelta, ev.data.scroll.ydelta);
                g_pending = SETTLE_FRAMES;
                break;
            case vr::VREvent_FocusLeave:
                io.AddFocusEvent(false);
                g_pending = SETTLE_FRAMES;
                break;
            case vr::VREvent_OverlayShown:
                shown();
                break;
            case vr::VREvent_OverlayHidden:
                g_visible = false;
                g_pending = 0;
                break;
            case vr::VREvent_Quit:
                g_system->AcknowledgeQuit_Exiting();
                return 1;
            default:
                break;
        }
    }
    if (!g_visible) {
        // A safety net for a missed OverlayShown, asked at most once a second.
        ULONGLONG now = GetTickCount64();
        if (now - g_checked < 1000) {
            return 0;
        }
        g_checked = now;
        if (!g_overlay->IsOverlayVisible(g_main)) {
            return 0;
        }
        shown();
    }
    if (g_pending > 0) {
        g_pending--;
        render();
    }
    return 0;
}

extern "C" int lfp_overlay_visible(void) {
    return g_visible ? 1 : 0;
}

// Releases whatever lfp_overlay_open got to, also after it failed halfway.
extern "C" void lfp_overlay_close(void) {
    if (g_imgui) {
        ImGui_ImplDX11_Shutdown();
        ImGui::DestroyContext();
        g_imgui = false;
    }
    if (g_overlay && g_main != vr::k_ulOverlayHandleInvalid) {
        g_overlay->DestroyOverlay(g_main);
    }
    g_main = vr::k_ulOverlayHandleInvalid;
    g_thumb = vr::k_ulOverlayHandleInvalid;
    if (g_target) g_target->Release();
    if (g_texture) g_texture->Release();
    if (g_context) g_context->Release();
    if (g_device) g_device->Release();
    g_target = nullptr;
    g_texture = nullptr;
    g_context = nullptr;
    g_device = nullptr;
}
