// src/utils.ts
import {invoke} from "@tauri-apps/api/core";

export async function invokeTauriCommandWrapper<T>(
    command: string,
    args: Record<string, unknown> | undefined,
    onSuccess: (result: T) => Promise<void> | void,
    onError: (errorMessage: string, rawError: unknown) => void
) {
    try {
        const result = await invoke<T>(command, args);
        const successResult = onSuccess(result);
        if (successResult instanceof Promise) {
            await successResult;
        }
    } catch (err) {
        const errorMessage = (typeof err === 'object' && err !== null && 'message' in err) ? String((err as {
            message: unknown
        }).message) : String(err);
        onError(errorMessage, err);
    }
}

// Decide whether a profile is a GPU/CUDA variant by name, so the torch-source picker
// keeps working even if the GPU profile is later renamed (cuda / nvidia-gpu / ...).
// Returns false for empty/undefined so it can be used against a maybe-null current_profile.
export const isGpuProfile = (name?: string | null): boolean => !!name && /(gpu|cuda)/i.test(name);
