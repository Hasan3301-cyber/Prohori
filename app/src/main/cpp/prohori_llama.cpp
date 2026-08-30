#include <jni.h>
#include <android/log.h>
#include <algorithm>
#include <atomic>
#include <chrono>
#include <dlfcn.h>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "ggml-backend.h"
#include "llama.h"

namespace {
std::mutex engine_mutex;
llama_model * cached_model = nullptr;
std::string cached_path;
std::once_flag backend_once;
// load µs, prompt µs, time-to-first-token µs, total µs, generated tokens, prompt tokens
jlong last_metrics[6] = {0, 0, 0, 0, 0, 0};
std::atomic_bool cancel_requested{false};

jlong elapsed_us(const std::chrono::steady_clock::time_point & start) {
    return std::chrono::duration_cast<std::chrono::microseconds>(
        std::chrono::steady_clock::now() - start
    ).count();
}

void throw_illegal_state(JNIEnv * env, const std::string & message) {
    jclass type = env->FindClass("java/lang/IllegalStateException");
    if (type != nullptr) env->ThrowNew(type, message.c_str());
}

std::string utf8(JNIEnv * env, jstring value) {
    if (value == nullptr) return {};
    const char * chars = env->GetStringUTFChars(value, nullptr);
    if (chars == nullptr) return {};
    std::string result(chars);
    env->ReleaseStringUTFChars(value, chars);
    return result;
}

bool load_model(const std::string & path, std::string & error) {
    if (cached_model != nullptr && cached_path == path) return true;
    if (cached_model != nullptr) {
        llama_model_free(cached_model);
        cached_model = nullptr;
        cached_path.clear();
    }

    llama_model_params params = llama_model_default_params();
    params.n_gpu_layers = 0;
    cached_model = llama_model_load_from_file(path.c_str(), params);
    if (cached_model == nullptr) {
        error = "Could not load the selected GGUF model";
        return false;
    }
    cached_path = path;
    return true;
}

std::vector<llama_token> tokenize(const llama_vocab * vocab, const std::string & text) {
    int count = llama_tokenize(vocab, text.c_str(), text.size(), nullptr, 0, true, true);
    if (count >= 0) return {};
    std::vector<llama_token> tokens(static_cast<size_t>(-count));
    count = llama_tokenize(vocab, text.c_str(), text.size(), tokens.data(), tokens.size(), true, true);
    if (count < 0) return {};
    tokens.resize(static_cast<size_t>(count));
    return tokens;
}

bool append_piece(const llama_vocab * vocab, llama_token token, std::string & output) {
    std::vector<char> buffer(256);
    int size = llama_token_to_piece(vocab, token, buffer.data(), buffer.size(), 0, true);
    if (size < 0) {
        buffer.resize(static_cast<size_t>(-size));
        size = llama_token_to_piece(vocab, token, buffer.data(), buffer.size(), 0, true);
    }
    if (size < 0) return false;
    output.append(buffer.data(), static_cast<size_t>(size));
    return true;
}

// A hybrid reasoning model can open a `<think>` block of its own even when the prompt
// prefills a closed one. A full stop inside that block is not the end of an answer, so the
// sentence stop must not fire there — otherwise chat returns reasoning and no answer.
bool inside_reasoning(const std::string & output) {
    const auto opened = output.rfind("<think>");
    if (opened == std::string::npos) return false;
    return output.find("</think>", opened) == std::string::npos;
}

// Chat asks for two or three short sentences, so let one form before stopping. The previous
// floor of six tokens ended the answer at the first full stop, which on this model was
// usually a fragment.
bool complete_short_answer(const std::string & output, uint32_t generated_count) {
    if (generated_count < 24 || output.size() < 120) return false;
    if (inside_reasoning(output)) return false;
    const auto last = output.find_last_not_of(" \t\r\n\"");
    if (last == std::string::npos) return false;
    return output[last] == '.' || output[last] == '!' || output[last] == '?';
}

struct deadline_state {
    std::chrono::steady_clock::time_point deadline;
};

/**
 * Length of the longest prefix of `text` that ends on a complete UTF-8 sequence.
 *
 * A tokenizer splits by token, not by codepoint, so one Bangla letter or one emoji can arrive
 * across two tokens. `NewStringUTF` on half a sequence is undefined behaviour, and the crash
 * it causes would land on the one screen someone is holding in an emergency. Streaming
 * therefore stops at the last whole character and lets the tail arrive with the next token.
 */
size_t complete_utf8_prefix(const std::string & text) {
    const size_t size = text.size();
    size_t position = size;
    // A trailing incomplete sequence is at most three continuation bytes long.
    for (int steps = 0; position > 0 && steps < 4; ++steps) {
        const auto byte = static_cast<unsigned char>(text[position - 1]);
        if ((byte & 0xC0) == 0x80) {
            --position;
            continue;
        }
        size_t needed = 1;
        if ((byte & 0xE0) == 0xC0) needed = 2;
        else if ((byte & 0xF0) == 0xE0) needed = 3;
        else if ((byte & 0xF8) == 0xF0) needed = 4;
        return size - (position - 1) >= needed ? size : position - 1;
    }
    return size;
}

/**
 * Hands each finished character to Kotlin while the model is still writing.
 *
 * The point is honesty about progress: a decode on a mid-range phone takes tens of seconds,
 * and a spinner for that long is indistinguishable from a hang. Text that grows is proof the
 * phone is working.
 *
 * The callback is invoked on the thread that called `generateNative`, which already holds
 * `engine_mutex`, so the Kotlin side must only record the text — never call back into the
 * engine. A callback that throws is reported once and then dropped: a broken listener must not
 * be able to abort a generation that is otherwise fine.
 */
class token_sink {
  public:
    token_sink(JNIEnv * env, jobject sink) : env_(env), sink_(sink) {
        if (sink_ == nullptr) return;
        jclass type = env_->GetObjectClass(sink_);
        if (type == nullptr) {
            env_->ExceptionClear();
            return;
        }
        method_ = env_->GetMethodID(type, "onToken", "(Ljava/lang/String;)V");
        env_->DeleteLocalRef(type);
        // A minifier that renamed the interface method must degrade to a silent spinner,
        // never to a crash on the screen someone is holding.
        if (method_ == nullptr) env_->ExceptionClear();
    }

    bool active() const { return sink_ != nullptr && method_ != nullptr && !broken_; }

    /** Emit whatever of `output` is newly complete. Safe to call after every token. */
    void flush(const std::string & output) {
        if (!active()) return;
        const size_t safe = complete_utf8_prefix(output);
        if (safe <= emitted_) return;
        const std::string piece = output.substr(emitted_, safe - emitted_);
        emitted_ = safe;
        jstring value = env_->NewStringUTF(piece.c_str());
        if (value == nullptr) {
            env_->ExceptionClear();
            broken_ = true;
            return;
        }
        env_->CallVoidMethod(sink_, method_, value);
        env_->DeleteLocalRef(value);
        if (env_->ExceptionCheck() == JNI_TRUE) {
            env_->ExceptionDescribe();
            env_->ExceptionClear();
            broken_ = true;
        }
    }

  private:
    JNIEnv * env_;
    jobject sink_;
    jmethodID method_ = nullptr;
    size_t emitted_ = 0;
    bool broken_ = false;
};

bool abort_after_deadline(void * data) {
    const auto * state = static_cast<const deadline_state *>(data);
    return cancel_requested.load(std::memory_order_relaxed) ||
        (state != nullptr && std::chrono::steady_clock::now() >= state->deadline);
}

void android_log_callback(enum ggml_log_level level, const char * text, void *) {
    int priority = ANDROID_LOG_INFO;
    if (level == GGML_LOG_LEVEL_ERROR) priority = ANDROID_LOG_ERROR;
    if (level == GGML_LOG_LEVEL_WARN) priority = ANDROID_LOG_WARN;
    __android_log_write(priority, "ProhoriLlama", text);
}

void initialize_backend() {
    llama_log_set(android_log_callback, nullptr);

    // Dynamic CPU variants live beside libprohori_llama.so inside the installed APK.
    // Android's linker namespace is deliberately narrow, so asking llama.cpp to scan an
    // implicit process directory can leave it on the unoptimized fallback or with no CPU
    // backend at all. Resolve our own library path and load variants from that exact dir.
    Dl_info info{};
    if (dladdr(reinterpret_cast<void *>(&initialize_backend), &info) != 0 && info.dli_fname != nullptr) {
        std::string library_path(info.dli_fname);
        const auto slash = library_path.find_last_of('/');
        if (slash != std::string::npos) {
            const std::string directory = library_path.substr(0, slash);
            ggml_backend_load_all_from_path(directory.c_str());
            __android_log_print(
                ANDROID_LOG_INFO,
                "ProhoriAI",
                "backend_scan dir=%s",
                directory.c_str()
            );
        }
    }
    llama_backend_init();

    // Say out loud which CPU backend actually won. With GGML_BACKEND_DL every backend is a
    // separate .so, so a packaging mistake shows up here as a device count of zero or as the
    // unoptimized baseline description — and a release build that silently lost its runtime
    // variants is otherwise indistinguishable from a slow phone.
    const size_t devices = ggml_backend_dev_count();
    if (devices == 0) {
        __android_log_write(
            ANDROID_LOG_ERROR,
            "ProhoriAI",
            "backend_none no compute device registered; check that libggml-cpu-*.so shipped"
        );
    }
    for (size_t index = 0; index < devices; ++index) {
        ggml_backend_dev_t device = ggml_backend_dev_get(index);
        if (device == nullptr) continue;
        __android_log_print(
            ANDROID_LOG_INFO,
            "ProhoriAI",
            "backend_device %zu/%zu name=%s description=%s",
            index + 1,
            devices,
            ggml_backend_dev_name(device),
            ggml_backend_dev_description(device)
        );
    }
}
} // namespace

extern "C" JNIEXPORT jstring JNICALL
Java_org_prohori_app_OnDeviceEngine_generateNative(
    JNIEnv * env,
    jobject,
    jstring model_path_value,
    jstring prompt_value,
    jstring grammar_value,
    jint max_output_tokens_value,
    jlong deadline_millis,
    jboolean stop_after_sentence_value,
    jobject sink_value
) {
    std::lock_guard<std::mutex> lock(engine_mutex);
    cancel_requested.store(false, std::memory_order_relaxed);
    std::call_once(backend_once, initialize_backend);
    std::fill_n(last_metrics, 6, 0);
    const auto started = std::chrono::steady_clock::now();

    const std::string model_path = utf8(env, model_path_value);
    const std::string prompt = utf8(env, prompt_value);
    const std::string grammar = utf8(env, grammar_value);
    if (
        model_path.empty() || prompt.empty() ||
        max_output_tokens_value < 1 || max_output_tokens_value > 384 ||
        deadline_millis < 1'000 || deadline_millis > 300'000
    ) {
        throw_illegal_state(env, "Model input and bounded generation settings are required");
        return nullptr;
    }
    const uint32_t max_output_tokens = static_cast<uint32_t>(max_output_tokens_value);
    const bool stop_after_sentence = stop_after_sentence_value == JNI_TRUE;
    const auto deadline = started + std::chrono::milliseconds(deadline_millis);
    deadline_state deadline_data{deadline};

    std::string error;
    if (!load_model(model_path, error)) {
        throw_illegal_state(env, error);
        return nullptr;
    }
    last_metrics[0] = elapsed_us(started);
    __android_log_print(
        ANDROID_LOG_INFO,
        "ProhoriAI",
        "model_ready load_ms=%lld",
        static_cast<long long>(last_metrics[0] / 1'000)
    );

    const llama_vocab * vocab = llama_model_get_vocab(cached_model);
    std::vector<llama_token> prompt_tokens = tokenize(vocab, prompt);
    __android_log_print(
        ANDROID_LOG_INFO,
        "ProhoriAI",
        "prompt_ready tokens=%zu",
        prompt_tokens.size()
    );
    if (prompt_tokens.empty() || prompt_tokens.size() + max_output_tokens > 4096) {
        throw_illegal_state(env, "The extraction prompt is too long for the 4096-token safety context");
        return nullptr;
    }

    llama_context_params context_params = llama_context_default_params();
    // Allocate only the KV cache this request can use. A fixed 4096-token cache cost
    // hundreds of megabytes on low-RAM phones even for a two-word chat message.
    const uint32_t required_context =
        static_cast<uint32_t>(prompt_tokens.size()) + max_output_tokens + 64;
    context_params.n_ctx = std::clamp(required_context, 256u, 4096u);
    context_params.n_batch = std::min<uint32_t>(256, static_cast<uint32_t>(prompt_tokens.size()));
    context_params.n_ubatch = context_params.n_batch;
    const unsigned cores = std::max(1u, std::thread::hardware_concurrency());
    context_params.n_threads = static_cast<int>(std::clamp(cores > 2 ? cores - 2 : cores, 1u, 4u));
    context_params.n_threads_batch = static_cast<int>(std::clamp(cores, 1u, 6u));
    context_params.abort_callback = abort_after_deadline;
    context_params.abort_callback_data = &deadline_data;
    context_params.no_perf = false;

    llama_context * context = llama_init_from_model(cached_model, context_params);
    if (context == nullptr) {
        // With GGML_BACKEND_DL a missing CPU variant also lands here, and blaming the phone's
        // memory for a packaging fault sends the owner to free up storage that was never the
        // problem. Distinguish the two.
        throw_illegal_state(
            env,
            ggml_backend_dev_count() == 0
                ? "This phone's CPU backend did not load, so the local model cannot run"
                : "Not enough memory to create the model context"
        );
        return nullptr;
    }

    llama_sampler * sampler = llama_sampler_chain_init(llama_sampler_chain_default_params());
    if (!grammar.empty()) {
        llama_sampler * grammar_sampler = llama_sampler_init_grammar(vocab, grammar.c_str(), "root");
        if (grammar_sampler == nullptr) {
            llama_sampler_free(sampler);
            llama_free(context);
            throw_illegal_state(env, "The bundled grammar is invalid for this model");
            return nullptr;
        }
        llama_sampler_chain_add(sampler, grammar_sampler);
    }
    // Qwen's published non-thinking recommendation is temp 0.7, top-k 20, top-p 0.8.
    // A fixed seed keeps eval runs reproducible while avoiding greedy repetition.
    llama_sampler_chain_add(sampler, llama_sampler_init_top_k(20));
    llama_sampler_chain_add(sampler, llama_sampler_init_top_p(0.8F, 1));
    llama_sampler_chain_add(sampler, llama_sampler_init_temp(0.7F));
    llama_sampler_chain_add(sampler, llama_sampler_init_dist(42));

    std::string output;
    uint32_t generated_count = 0;
    token_sink sink(env, sink_value);
    // llama_decode requires each submitted batch to fit n_batch. Structured medical
    // prompts are intentionally detailed and can exceed the 256-token physical batch;
    // passing all tokens at once made llama.cpp abort the Android process. Prefill in
    // bounded chunks. With implicit positions llama.cpp continues from the KV cache.
    size_t prompt_offset = 0;
    while (prompt_offset < prompt_tokens.size()) {
        const auto chunk_size = static_cast<int32_t>(std::min<size_t>(
            context_params.n_batch,
            prompt_tokens.size() - prompt_offset
        ));
        llama_batch prompt_batch = llama_batch_get_one(
            prompt_tokens.data() + prompt_offset,
            chunk_size
        );
        if (llama_decode(context, prompt_batch) != 0) {
            const bool cancelled = cancel_requested.load(std::memory_order_relaxed);
            const bool timed_out = std::chrono::steady_clock::now() >= deadline;
            error =
                cancelled
                    ? "Local AI request cancelled"
                    : timed_out
                    ? "The local model reached this phone's time limit"
                    : "The model failed while evaluating the prompt";
            break;
        }
        prompt_offset += static_cast<size_t>(chunk_size);
    }
    if (error.empty()) last_metrics[1] = elapsed_us(started) - last_metrics[0];

    for (uint32_t generated = 0; error.empty() && generated < max_output_tokens; ++generated) {
        llama_token token = llama_sampler_sample(sampler, context, -1);
        if (llama_vocab_is_eog(vocab, token)) break;
        if (!append_piece(vocab, token, output)) {
            error = "The model emitted an unreadable token";
            break;
        }
        ++generated_count;
        if (generated_count == 1) last_metrics[2] = elapsed_us(started);
        // Hand over whatever is now a whole character. This happens before every stop check
        // below so that text the model did produce is on screen even when the next line ends
        // the run — a deadline that arrives mid-answer should shorten the answer, not erase it.
        sink.flush(output);
        if (stop_after_sentence && complete_short_answer(output, generated_count)) break;
        if (std::chrono::steady_clock::now() >= deadline) {
            if (output.empty()) error = "The local model did not produce a token before the time limit";
            break;
        }
        if (generated + 1 < max_output_tokens) {
            llama_batch token_batch = llama_batch_get_one(&token, 1);
            if (llama_decode(context, token_batch) != 0) {
                const bool cancelled = cancel_requested.load(std::memory_order_relaxed);
                const bool timed_out = std::chrono::steady_clock::now() >= deadline;
                // A slow phone can cross the deadline after producing useful chat text.
                // Keep that text; structured JSON still fails closed in Rust if incomplete.
                if (!(timed_out && !cancelled && stop_after_sentence && !output.empty())) {
                    error =
                        cancelled
                            ? "Local AI request cancelled"
                            : timed_out
                            ? "The local model reached this phone's time limit"
                            : "The model failed while generating an answer";
                }
                break;
            }
        }
    }

    last_metrics[3] = elapsed_us(started);
    last_metrics[4] = static_cast<jlong>(generated_count);
    last_metrics[5] = static_cast<jlong>(prompt_tokens.size());

    __android_log_print(
        ANDROID_LOG_INFO,
        "ProhoriAI",
        "inference prompt=%lld generated=%lld total_ms=%lld",
        static_cast<long long>(last_metrics[5]),
        static_cast<long long>(last_metrics[4]),
        static_cast<long long>(last_metrics[3] / 1'000)
    );

    llama_sampler_free(sampler);
    llama_free(context);
    if (!error.empty()) {
        throw_illegal_state(env, error);
        return nullptr;
    }
    if (output.empty()) {
        throw_illegal_state(env, "The model returned no structured assessment");
        return nullptr;
    }
    return env->NewStringUTF(output.c_str());
}

extern "C" JNIEXPORT void JNICALL
Java_org_prohori_app_OnDeviceEngine_cancelNative(JNIEnv *, jobject) {
    // Deliberately does not take engine_mutex: generation owns that mutex, and cancellation
    // must be observable by llama.cpp's abort callback while generation is still running.
    cancel_requested.store(true, std::memory_order_relaxed);
}

extern "C" JNIEXPORT jlongArray JNICALL
Java_org_prohori_app_OnDeviceEngine_lastMetricsNative(JNIEnv * env, jobject) {
    std::lock_guard<std::mutex> lock(engine_mutex);
    jlongArray result = env->NewLongArray(6);
    if (result != nullptr) env->SetLongArrayRegion(result, 0, 6, last_metrics);
    return result;
}
