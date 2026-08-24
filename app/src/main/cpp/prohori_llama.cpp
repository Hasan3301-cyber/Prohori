#include <jni.h>
#include <android/log.h>
#include <algorithm>
#include <chrono>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "llama.h"

namespace {
std::mutex engine_mutex;
llama_model * cached_model = nullptr;
std::string cached_path;
std::once_flag backend_once;
// load µs, prompt µs, time-to-first-token µs, total µs, generated tokens, prompt tokens
jlong last_metrics[6] = {0, 0, 0, 0, 0, 0};

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

bool complete_short_answer(const std::string & output, uint32_t generated_count) {
    if (generated_count < 6 || output.size() < 18) return false;
    const auto last = output.find_last_not_of(" \t\r\n\"");
    if (last == std::string::npos) return false;
    return output[last] == '.' || output[last] == '!' || output[last] == '?';
}

struct deadline_state {
    std::chrono::steady_clock::time_point deadline;
};

bool abort_after_deadline(void * data) {
    const auto * state = static_cast<const deadline_state *>(data);
    return state != nullptr && std::chrono::steady_clock::now() >= state->deadline;
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
    jboolean stop_after_sentence_value
) {
    std::lock_guard<std::mutex> lock(engine_mutex);
    std::call_once(backend_once, [] { llama_backend_init(); });
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
        throw_illegal_state(env, "Not enough memory to create the model context");
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

    llama_batch batch = llama_batch_get_one(prompt_tokens.data(), prompt_tokens.size());
    std::string output;
    uint32_t generated_count = 0;
    for (uint32_t generated = 0; generated < max_output_tokens; ++generated) {
        if (llama_decode(context, batch) != 0) {
            error =
                std::chrono::steady_clock::now() >= deadline
                    ? "The local model reached this phone's one-minute time limit"
                    : "The model failed while evaluating the prompt";
            break;
        }
        if (generated == 0) last_metrics[1] = elapsed_us(started) - last_metrics[0];
        llama_token token = llama_sampler_sample(sampler, context, -1);
        if (llama_vocab_is_eog(vocab, token)) break;
        if (!append_piece(vocab, token, output)) {
            error = "The model emitted an unreadable token";
            break;
        }
        ++generated_count;
        if (generated_count == 1) last_metrics[2] = elapsed_us(started);
        if (stop_after_sentence && complete_short_answer(output, generated_count)) break;
        if (std::chrono::steady_clock::now() >= deadline) {
            if (output.empty()) error = "The local model did not produce a token before the time limit";
            break;
        }
        batch = llama_batch_get_one(&token, 1);
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

extern "C" JNIEXPORT jlongArray JNICALL
Java_org_prohori_app_OnDeviceEngine_lastMetricsNative(JNIEnv * env, jobject) {
    std::lock_guard<std::mutex> lock(engine_mutex);
    jlongArray result = env->NewLongArray(6);
    if (result != nullptr) env->SetLongArrayRegion(result, 0, 6, last_metrics);
    return result;
}
