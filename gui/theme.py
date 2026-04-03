"""Shared colour and layout constants for all GUI pages."""

# ── Colours ───────────────────────────────────────────────────────────────────

BG     = "#0f0f1a"
PANEL  = "#1a1a2e"
BORDER = "#2e2e50"
ACCENT = "#5b8dee"
TEXT   = "#ccccdd"
MUTED  = "#7777aa"

# ── Font sizes ────────────────────────────────────────────────────────────────

FONT_TITLE      = 34   # page-level hero titles
FONT_HEADING    = 18   # dialog / major section titles
FONT_SUBHEADING = 15   # card titles, list headers
FONT_BODY       = 13   # primary content / button labels
FONT_SECONDARY  = 12   # labels, secondary metadata
FONT_TERTIARY   = 11   # muted text, tags, badges

# ── Spacing scale ─────────────────────────────────────────────────────────────

SPACE_XL  = 28   # between major page sections
SPACE_LG  = 20   # between cards / groups
SPACE_MD  = 12   # between related elements
SPACE_SM  = 8    # tight spacing (button rows, label-input pairs)
SPACE_XS  = 4    # minimal gap

# ── Border radii ──────────────────────────────────────────────────────────────

RADIUS_LG = 10   # cards, panels, dialogs
RADIUS_MD = 6    # buttons, text inputs
RADIUS_SM = 4    # small buttons, badges, chips

# ── Button heights ────────────────────────────────────────────────────────────

BTN_H_LG = 36   # primary action buttons (Save, Create)
BTN_H_MD = 32   # standard toolbar / filter buttons
BTN_H_SM = 28   # compact / icon-adjacent buttons

# ── Page / panel geometry ─────────────────────────────────────────────────────

PAGE_MARGIN_H  = 48   # horizontal outer margin on all pages
PAGE_MARGIN_V  = 36   # vertical outer margin on all pages
CARD_PAD_H     = 16   # horizontal padding inside cards
CARD_PAD_V     = 12   # vertical padding inside cards
DIALOG_PAD     = 20   # padding inside dialog content areas

# ── Fixed widget sizes ────────────────────────────────────────────────────────

NAV_WIDTH       = 120   # sidebar navigation width
NOTE_HEIGHT     = 150   # note preview (MarkdownView) height
ABSTRACT_HEIGHT = 200   # abstract / summary view height
