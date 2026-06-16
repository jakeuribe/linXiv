import type { SelectHTMLAttributes } from "react";

type SelectSize = "sm" | "md";

/** Inline density. "sm" sits next to text-xs labels without dwarfing them;
 *  "md" (default) is the standalone form-control size. */
const sizeStyles: Record<SelectSize, string> = {
  sm: "px-1.5 py-0.5 text-xs",
  // Matches the Input/Textarea metrics (px-3 py-1.5 text-sm) so a select reads
  // as a peer of adjacent text fields and their content aligns.
  md: "px-3 py-1.5 text-sm",
};

// Native <select> has a numeric `size` (visible rows); omit it so we can reuse
// the conventional `size` prop name for density, matching Button/Badge.
interface SelectProps
  extends Omit<SelectHTMLAttributes<HTMLSelectElement>, "size"> {
  size?: SelectSize;
}

/** Styled wrapper around a native <select>, matching the Input/Textarea/Badge
 *  primitives. Callers pass <option> children. Use `size` for inline density;
 *  `className` is appended, so avoid passing padding/size utilities that
 *  conflict with the variant (the project has no tailwind-merge to resolve
 *  such conflicts). */
export function Select({
  size = "md",
  className = "",
  ...props
}: SelectProps) {
  return (
    <select
      className={[
        "rounded-md border border-[var(--color-border)] bg-[var(--color-panel)]",
        "text-[var(--color-text)] cursor-pointer",
        "disabled:opacity-60 disabled:cursor-not-allowed",
        sizeStyles[size],
        className,
      ].join(" ")}
      {...props}
    />
  );
}

interface OptionSelectOption<T extends string> {
  value: T;
  label: string;
}

interface OptionSelectBaseProps<T extends string> {
  options: OptionSelectOption<T>[];
  value: T;
  onChange: (value: T) => void;
  placeholder?: string;
  disabled?: boolean;
  size?: SelectSize;
  className?: string;
}

type OptionSelectProps<T extends string> = OptionSelectBaseProps<T> &
  (
    | { id: string; "aria-label"?: string }
    | { id?: string; "aria-label": string }
  );

export function OptionSelect<T extends string>({
  options,
  value,
  onChange,
  placeholder,
  disabled,
  size = "md",
  className = "",
  id,
  "aria-label": ariaLabel,
}: OptionSelectProps<T>) {
  return (
    <select
      id={id}
      aria-label={ariaLabel}
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value as T)}
      className={[
        "min-w-[180px] rounded-md border border-[var(--color-border)] bg-[var(--color-panel)]",
        "text-[var(--color-text)] cursor-pointer",
        "disabled:opacity-60 disabled:cursor-not-allowed",
        sizeStyles[size],
        className,
      ].join(" ")}
    >
      {placeholder !== undefined && (
        <option value="" disabled>
          {placeholder}
        </option>
      )}
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  );
}
