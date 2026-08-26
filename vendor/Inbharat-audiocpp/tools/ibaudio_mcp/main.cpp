// ibaudio-mcp: MCP (Model Context Protocol) gateway over InBharat Audio.
//
// Transport: stdio (newline-delimited JSON-RPC), the standard binding for a
// client-launched subprocess. Dual-era: modern 2026-07-28 requests carry
// protocol metadata in _meta and are served statelessly; a legacy `initialize`
// handshake is answered for older clients.
//
// Scope: CONTROL ONLY. Tools expose capabilities, model catalog, language
// detection on text, diagnostics, and lifecycle. Continuous PCM audio never
// travels over MCP — live voice uses the native streaming API / transport.
// Everything here goes through the public C ABI; no engine internals are touched.
//
// Dependency-free: a minimal purpose-built JSON reader/writer (no external JSON
// library), consistent with the project's no-dependency rule.

#include "inbharat/ibaudio.h"

#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <map>
#include <sstream>
#include <string>
#include <vector>

namespace {

// --- Minimal JSON value ---------------------------------------------------------
// Only what the MCP control plane needs: objects, arrays, strings, numbers, bools,
// null. Numbers are kept as their source text to avoid float round-trips.
struct Json {
    enum Type { Null, Bool, Num, Str, Arr, Obj } type = Null;
    bool boolean = false;
    std::string text;                       // Num raw text or Str value
    std::vector<Json> arr;
    std::map<std::string, Json> obj;

    const Json *find(const std::string &key) const {
        if (type != Obj) return nullptr;
        auto it = obj.find(key);
        return it == obj.end() ? nullptr : &it->second;
    }
    std::string str(const std::string &fallback = "") const {
        return type == Str ? text : fallback;
    }
};

struct Parser {
    const char *p;
    explicit Parser(const std::string &s) : p(s.c_str()) {}
    void ws() { while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') ++p; }
    bool consume(char c) { ws(); if (*p == c) { ++p; return true; } return false; }

    Json value() {
        ws();
        if (*p == '{') return object();
        if (*p == '[') return array();
        if (*p == '"') { Json j; j.type = Json::Str; j.text = string(); return j; }
        if (*p == 't' && std::strncmp(p, "true", 4) == 0) { p += 4; Json j; j.type = Json::Bool; j.boolean = true; return j; }
        if (*p == 'f' && std::strncmp(p, "false", 5) == 0) { p += 5; Json j; j.type = Json::Bool; return j; }
        if (*p == 'n' && std::strncmp(p, "null", 4) == 0) { p += 4; return Json{}; }
        return number();
    }
    Json number() {
        const char *start = p;
        if (*p == '-') ++p;
        while ((*p >= '0' && *p <= '9') || *p == '.' || *p == 'e' || *p == 'E' || *p == '+' || *p == '-') ++p;
        Json j; j.type = Json::Num; j.text.assign(start, p - start); return j;
    }
    std::string string() {
        std::string out;
        ++p;  // opening quote
        while (*p && *p != '"') {
            if (*p == '\\' && p[1]) {
                ++p;
                switch (*p) {
                    case 'n': out += '\n'; break;
                    case 't': out += '\t'; break;
                    case 'r': out += '\r'; break;
                    case '"': out += '"'; break;
                    case '\\': out += '\\'; break;
                    case '/': out += '/'; break;
                    case 'u': {  // \uXXXX -> UTF-8 (BMP only; surrogate pairs -> U+FFFD)
                        unsigned code = 0;
                        for (int k = 0; k < 4 && p[1]; ++k) {
                            ++p; char h = *p; code <<= 4;
                            code |= (h >= '0' && h <= '9') ? (h - '0') : ((h | 32) >= 'a' && (h | 32) <= 'f' ? (h | 32) - 'a' + 10 : 0);
                        }
                        if (code < 0x80) out += static_cast<char>(code);
                        else if (code < 0x800) { out += static_cast<char>(0xC0 | (code >> 6)); out += static_cast<char>(0x80 | (code & 0x3F)); }
                        else if (code >= 0xD800 && code <= 0xDFFF) { out += "\xEF\xBF\xBD"; }
                        else { out += static_cast<char>(0xE0 | (code >> 12)); out += static_cast<char>(0x80 | ((code >> 6) & 0x3F)); out += static_cast<char>(0x80 | (code & 0x3F)); }
                        break;
                    }
                    default: out += *p;
                }
                ++p;
            } else {
                out += *p++;
            }
        }
        if (*p == '"') ++p;
        return out;
    }
    Json array() {
        Json j; j.type = Json::Arr;
        ++p;
        if (consume(']')) return j;
        while (*p) { j.arr.push_back(value()); if (consume(']')) break; consume(','); }
        return j;
    }
    Json object() {
        Json j; j.type = Json::Obj;
        ++p;
        if (consume('}')) return j;
        while (*p) {
            ws();
            std::string key = (*p == '"') ? string() : std::string();
            consume(':');
            j.obj[key] = value();
            if (consume('}')) break;
            consume(',');
        }
        return j;
    }
};

// --- JSON writer ----------------------------------------------------------------
void escape_into(std::string &out, const std::string &s) {
    for (unsigned char c : s) {
        switch (c) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (c < 0x20) { char b[8]; std::snprintf(b, sizeof b, "\\u%04x", c); out += b; }
                else out += static_cast<char>(c);
        }
    }
}
std::string jstr(const std::string &s) { std::string o = "\""; escape_into(o, s); return o + "\""; }
std::string jraw_id(const Json &id) {
    if (id.type == Json::Str) return jstr(id.text);
    if (id.type == Json::Num) return id.text;
    return "null";
}

// --- Runtime --------------------------------------------------------------------
ibaudio_runtime_t *g_runtime = nullptr;

ibaudio_runtime_t *runtime() {
    if (g_runtime == nullptr) {
        ibaudio_runtime_options_v1 options{};
        ibaudio_runtime_options_init(&options);
        if (ibaudio_runtime_create(&options, &g_runtime) != IBAUDIO_STATUS_OK) g_runtime = nullptr;
    }
    return g_runtime;
}

std::string capabilities_json() {
    ibaudio_capabilities_v1 caps{};
    caps.struct_size = sizeof(caps);
    caps.api_version = IBAUDIO_API_VERSION;
    if (ibaudio_runtime_get_capabilities(runtime(), &caps) != IBAUDIO_STATUS_OK) return "{}";
    std::ostringstream o;
    o << "{\"api_version\":" << ibaudio_get_api_version()
      << ",\"runtime_version\":\"" << ibaudio_get_runtime_version() << "\""
      << ",\"abi_major\":" << caps.abi_major << ",\"abi_minor\":" << caps.abi_minor
      << ",\"model_count\":" << caps.model_count << ",\"backend_count\":" << caps.backend_count
      << ",\"max_input_frames\":" << caps.max_input_frames
      << ",\"feature_flags\":" << caps.feature_flags << "}";
    return o.str();
}

std::string models_json() {
    uint32_t count = 0;
    if (ibaudio_runtime_get_model_count(runtime(), &count) != IBAUDIO_STATUS_OK) return "[]";
    std::ostringstream o;
    o << "[";
    for (uint32_t i = 0; i < count; ++i) {
        ibaudio_model_descriptor_v1 d{};
        d.struct_size = sizeof(d);
        d.api_version = IBAUDIO_API_VERSION;
        if (ibaudio_runtime_get_model_descriptor(runtime(), i, &d) != IBAUDIO_STATUS_OK) continue;
        if (i) o << ",";
        o << "{\"id\":\"" << d.id << "\",\"family\":\"" << d.family << "\""
          << ",\"task\":" << d.task << ",\"available\":" << (d.available ? "true" : "false")
          << ",\"streaming_label\":\"" << d.streaming_label << "\""
          << ",\"availability_reason\":\"" << d.availability_reason << "\"}";
    }
    o << "]";
    return o.str();
}

std::string diagnostics_json() {
    ibaudio_buffer_t *diag = nullptr;
    if (ibaudio_runtime_get_diagnostics_json(runtime(), &diag) != IBAUDIO_STATUS_OK || diag == nullptr) return "{}";
    const void *data = nullptr;
    uint64_t size = 0;
    std::string s;
    if (ibaudio_buffer_get_data(diag, &data, &size) == IBAUDIO_STATUS_OK && data != nullptr) {
        s.assign(static_cast<const char *>(data), static_cast<size_t>(size));
    }
    ibaudio_buffer_release(&diag);
    return s.empty() ? "{}" : s;
}

std::string detect_language_json(const std::string &text) {
    // Route transcript text through the code-switch detector (script-ratio heuristic).
    ibaudio_codeswitch_detector_t *detector = ibaudio_codeswitch_detector_create();
    if (detector == nullptr) return "{\"error\":\"detector unavailable\"}";
    ibaudio_language_score_v1 score{};
    score.struct_size = sizeof(score);
    score.api_version = IBAUDIO_API_VERSION;
    const ibaudio_status_t st = ibaudio_codeswitch_detector_detect(detector, text.c_str(), nullptr, &score);
    std::ostringstream o;
    if (st == IBAUDIO_STATUS_OK) {
        o << "{\"english\":" << score.english << ",\"hindi\":" << score.hindi
          << ",\"hinglish\":" << score.hinglish << ",\"confidence\":" << score.confidence
          << ",\"note\":\"script-ratio heuristic, not acoustic LID\"}";
    } else {
        o << "{\"error\":\"detect failed\"}";
    }
    ibaudio_codeswitch_detector_destroy(detector);
    return o.str();
}

std::string language_packs_json(const std::string &root_text) {
    const std::filesystem::path root = root_text.empty() ? std::filesystem::path("packs")
                                                         : std::filesystem::path(root_text);
    const std::filesystem::path catalog = root / "catalog.v1.tsv";
    std::ifstream input(catalog);
    if (!input) return "{\"error\":\"language-pack catalog not found\",\"root\":" + jstr(root.string()) + "}";
    std::ostringstream out;
    out << "{\"schema\":\"inbharat.language-pack-catalog.v1\",\"packs\":[";
    std::string line;
    bool first = true;
    while (std::getline(input, line)) {
        if (line.empty() || line.front() == '#') continue;
        std::vector<std::string> fields;
        std::stringstream row(line);
        std::string field;
        while (std::getline(row, field, '\t')) fields.push_back(field);
        if (fields.size() != 4) return "{\"error\":\"malformed language-pack catalog\"}";
        if (!first) out << ',';
        first = false;
        out << "{\"language\":" << jstr(fields[0])
            << ",\"manifest\":" << jstr(fields[1])
            << ",\"sha256\":" << jstr(fields[2])
            << ",\"scripts\":" << jstr(fields[3]) << "}";
    }
    out << "]}";
    return out.str();
}

// --- MCP plumbing ----------------------------------------------------------------
std::string make_result(const Json &id, const std::string &result_payload) {
    return "{\"jsonrpc\":\"2.0\",\"id\":" + jraw_id(id) + ",\"result\":" + result_payload + "}";
}
std::string make_error(const Json &id, int code, const std::string &message) {
    return "{\"jsonrpc\":\"2.0\",\"id\":" + jraw_id(id) +
           ",\"error\":{\"code\":" + std::to_string(code) + ",\"message\":" + jstr(message) + "}}";
}
std::string tool_text_result(const Json &id, const std::string &text) {
    return make_result(id, "{\"content\":[{\"type\":\"text\",\"text\":" + jstr(text) + "}]}");
}

const char *TOOLS_LIST =
    "{\"tools\":["
    "{\"name\":\"audio.capabilities\",\"description\":\"Runtime capabilities and feature flags\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}}},"
    "{\"name\":\"audio.models\",\"description\":\"List registered models with task, availability, and streaming class\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}}},"
    "{\"name\":\"audio.detect_language\",\"description\":\"Script-ratio language-mix estimate for transcript text (not acoustic LID)\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}},\"required\":[\"text\"]}},"
    "{\"name\":\"audio.language_packs\",\"description\":\"List the hash-pinned 22-language pack catalog and scripts; task support remains evidence-gated\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"root\":{\"type\":\"string\"}}}},"
    "{\"name\":\"audio.health\",\"description\":\"Diagnostics snapshot\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}}}"
    "]}";

const char *RESOURCES_LIST =
    "{\"resources\":["
    "{\"uri\":\"ibaudio://capabilities\",\"name\":\"Runtime capabilities\"},"
    "{\"uri\":\"ibaudio://models\",\"name\":\"Registered model catalog\"},"
    "{\"uri\":\"ibaudio://language-packs\",\"name\":\"Hash-pinned 22-language pack catalog\"},"
    "{\"uri\":\"ibaudio://metrics\",\"name\":\"Diagnostics and metrics\"}"
    "]}";

std::string handle_request(const Json &req) {
    Json empty_id;  // null id for notifications we choose not to answer
    const Json *id = req.find("id");
    const Json *method = req.find("method");
    const std::string m = method != nullptr ? method->str() : "";
    const Json the_id = id != nullptr ? *id : empty_id;

    // Modern stateless discovery.
    if (m == "server/discover" || m == "initialize") {
        return make_result(the_id,
            "{\"protocolVersion\":\"2026-07-28\","
            "\"serverInfo\":{\"name\":\"ibaudio-mcp\",\"version\":\"0.1.0\"},"
            "\"capabilities\":{\"tools\":{},\"resources\":{}}}");
    }
    if (m == "notifications/initialized" || m == "notifications/cancelled") {
        return std::string();  // notifications: no response
    }
    if (m == "tools/list") return make_result(the_id, TOOLS_LIST);
    if (m == "resources/list") return make_result(the_id, RESOURCES_LIST);
    if (m == "resources/read") {
        const Json *params = req.find("params");
        const std::string uri = params != nullptr && params->find("uri") != nullptr
                                ? params->find("uri")->str() : "";
        std::string body = "{}";
        std::string mime = "application/json";
        if (uri == "ibaudio://capabilities") body = capabilities_json();
        else if (uri == "ibaudio://models") body = models_json();
        else if (uri == "ibaudio://language-packs") body = language_packs_json("packs");
        else if (uri == "ibaudio://metrics") body = diagnostics_json();
        else return make_error(the_id, -32602, "unknown resource uri");
        return make_result(the_id,
            "{\"contents\":[{\"uri\":" + jstr(uri) + ",\"mimeType\":\"" + mime + "\",\"text\":" + jstr(body) + "}]}");
    }
    if (m == "tools/call") {
        const Json *params = req.find("params");
        const std::string name = params != nullptr && params->find("name") != nullptr
                                 ? params->find("name")->str() : "";
        if (name == "audio.capabilities") return tool_text_result(the_id, capabilities_json());
        if (name == "audio.models") return tool_text_result(the_id, models_json());
        if (name == "audio.health") return tool_text_result(the_id, diagnostics_json());
        if (name == "audio.language_packs") {
            const Json *args = params != nullptr ? params->find("arguments") : nullptr;
            const std::string root = args != nullptr && args->find("root") != nullptr
                                     ? args->find("root")->str() : "packs";
            return tool_text_result(the_id, language_packs_json(root));
        }
        if (name == "audio.detect_language") {
            const Json *args = params != nullptr ? params->find("arguments") : nullptr;
            const std::string text = args != nullptr && args->find("text") != nullptr
                                     ? args->find("text")->str() : "";
            if (text.empty()) return make_error(the_id, -32602, "audio.detect_language requires 'text'");
            return tool_text_result(the_id, detect_language_json(text));
        }
        return make_error(the_id, -32601, "unknown tool: " + name);
    }
    if (m.empty()) return make_error(the_id, -32600, "invalid request: missing method");
    return make_error(the_id, -32601, "method not found: " + m);
}

} // namespace

int main() {
    if (runtime() == nullptr) {
        std::fprintf(stderr, "ibaudio-mcp: failed to create runtime\n");
        return 1;
    }
    std::string line;
    while (std::getline(std::cin, line)) {
        if (line.empty()) continue;
        Parser parser(line);
        Json req = parser.value();
        const std::string response = handle_request(req);
        if (!response.empty()) {
            std::cout << response << "\n" << std::flush;
        }
    }
    if (g_runtime != nullptr) ibaudio_runtime_release(&g_runtime);
    return 0;
}
