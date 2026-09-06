import type { ComputedRef, InjectionKey } from 'vue';

/** Shared by a Field and its control, including controls nested in a path picker. */
export interface FieldContext {
    controlId: string;
    labelId: string;
    descriptionId: ComputedRef<string | undefined>;
    required: ComputedRef<boolean>;
    invalid: ComputedRef<boolean>;
}

export const fieldKey: InjectionKey<FieldContext> = Symbol('field');
