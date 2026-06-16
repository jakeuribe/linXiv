import { useEffect, useRef } from "react";
import type { KeyboardEvent } from "react";

interface SegmentedOption<T extends string> {
  value: T;
  label: string;
}

interface SegmentedProps<T extends string> {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  disabled?: boolean;
  className?: string;
  "aria-label": string;
}

export function Segmented<T extends string>({
  options,
  value,
  onChange,
  disabled,
  className = "",
  "aria-label": ariaLabel,
}: SegmentedProps<T>) {
  const buttonsRef = useRef<(HTMLButtonElement | null)[]>([]);
  useEffect(() => {
    buttonsRef.current.length = options.length;
  }, [options.length]);

  const activeIndex = options.findIndex((o) => o.value === value);
  const hasSelection = activeIndex !== -1;

  const select = (next: T) => {
    if (next !== value) onChange(next);
  };

  const focusIndex = (index: number) => {
    const clamped = (index + options.length) % options.length;
    buttonsRef.current[clamped]?.focus();
    select(options[clamped].value);
  };

  const handleKeyDown = (
    event: KeyboardEvent<HTMLButtonElement>,
    index: number,
  ) => {
    if (disabled) return;
    switch (event.key) {
      case "ArrowRight":
      case "ArrowDown":
        event.preventDefault();
        focusIndex(index + 1);
        break;
      case "ArrowLeft":
      case "ArrowUp":
        event.preventDefault();
        focusIndex(index - 1);
        break;
      case "Home":
        event.preventDefault();
        focusIndex(0);
        break;
      case "End":
        event.preventDefault();
        focusIndex(options.length - 1);
        break;
    }
  };

  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      className={[
        "inline-flex border-b border-[var(--color-border)]",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      {options.map((option, index) => {
        const active = option.value === value;
        return (
          <button
            key={option.value}
            ref={(el) => {
              buttonsRef.current[index] = el;
            }}
            type="button"
            role="radio"
            aria-checked={active}
            tabIndex={active ? 0 : !hasSelection && index === 0 ? 0 : -1}
            disabled={disabled}
            onClick={() => select(option.value)}
            onKeyDown={(event) => handleKeyDown(event, index)}
            className={[
              "px-4 py-2 text-sm font-medium -mb-px border-b-2 border-transparent",
              "transition-colors",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-accent)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--color-bg)]",
              "disabled:opacity-50 disabled:cursor-not-allowed",
              active
                ? "text-[var(--color-text)] border-[var(--color-accent)]"
                : "text-[var(--color-muted)] hover:text-[var(--color-text)]",
            ].join(" ")}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
