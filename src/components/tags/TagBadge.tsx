import { Link } from "react-router";
import { Badge } from "../ui/badge";

interface TagBadgeProps {
  label: string;
  className?: string;
}

export function TagBadge({ label, className }: TagBadgeProps) {
  return (
    <Link
      to={`/tags/${encodeURIComponent(label)}`}
      onClick={(e) => e.stopPropagation()}
      className="inline-flex"
    >
      <Badge
        className={[
          "hover:opacity-80 transition-opacity",
          className ?? "",
        ].join(" ").trim()}
      >
        {label}
      </Badge>
    </Link>
  );
}
