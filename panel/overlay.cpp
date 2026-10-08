// The dashboard tab: a D3D11 render target drawn by Dear ImGui, handed to SteamVR as the
// overlay's texture, with the laser pointer's events fed back to ImGui as a mouse. No window.
#include "panel.h"

#include <windows.h>
#include <d3d11.h>
#include <openvr.h>

#include "imgui.h"
#include "backends/imgui_impl_dx11.h"

#include <cstdio>

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
bool g_clicked;

}  // namespace

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
    ImGui_ImplDX11_Init(g_device, g_context);
    return 0;
}

// Draws one frame of the panel. Build 2: the tab itself, with one button to prove the pointer.
static void draw() {
    ImGui::SetNextWindowPos(ImVec2(0, 0));
    ImGui::SetNextWindowSize(ImGui::GetIO().DisplaySize);
    ImGui::Begin("LeapFrog", nullptr, ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoResize | ImGuiWindowFlags_NoSavedSettings);
    ImGui::Text("LeapFrog %s", LFP_VERSION);
    ImGui::Separator();
    ImGui::TextWrapped("The settings follow in the next build. This tab only checks that the panel shows up and takes the pointer.");
    ImGui::Dummy(ImVec2(0, 20));
    if (ImGui::Button(g_clicked ? "Clicked, click again" : "Click me", ImVec2(360, 80))) {
        g_clicked = !g_clicked;
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
