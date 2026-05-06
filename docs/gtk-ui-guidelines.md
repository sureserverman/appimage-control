# GTK UI/UX Guidelines for AI Agents

Practical guidance for building classy GTK apps that feel native on Ubuntu GNOME and still look disciplined on LXQt.

Research basis: GNOME HIG, libadwaita docs, Ubuntu Yaru theme notes, and LXQt theming documentation. This guide favors GTK 4 + libadwaita patterns because that is the current primary GTK design stack. Where a rule is an inference for cross-desktop portability rather than a direct GTK/GNOME requirement, it is labeled as such.

## Design Target

Optimize for this outcome:

- Feels fully native on GNOME and Ubuntu's Yaru-derived GTK styling.
- Remains clean, legible, and proportional under non-GNOME GTK themes.
- Avoids custom CSS and chrome that assume one exact theme, one accent color, or one compositor behavior.

In practice, that means:

- Prefer standard GTK/libadwaita widgets over custom-painted UI.
- Prefer semantic structure over decorative styling.
- Let the active theme do most of the visual work.

## Non-Negotiable Rules

1. Use GTK 4 with libadwaita widgets and patterns when possible.
2. Make all primary windows resizable, with default sizes matched to content rather than oversized empty space.
3. Use header bars, boxed lists, sidebars, dialogs, and menus the way GNOME HIG expects.
4. Hardcode as little visual styling as possible.
5. Support light, dark, and high-contrast modes without breakage.
6. Keep layouts adaptive so narrow windows still work.
7. Use one clear primary action per view, not multiple competing calls to action.

## Windows

- Use one main `AdwApplicationWindow` per primary task area.
- Primary windows should be independent. Closing one should not implicitly close other primary windows.
- Default window size should match the task:
  - document or media views can open larger;
  - utility and preferences windows should open smaller.
- Restore previous window size when restoring the same view/content state.
- Support standard close shortcuts: `Ctrl+W` for windows, `Esc` for modal windows.

Design implication for agents:

- Start with a single clear content column or a sidebar-plus-content split, not a dashboard full of panels.
- Avoid tiny utility panes unless they collapse cleanly at narrow widths.

## Header Bars

- Put primary actions at the start of the header bar, title or view switcher in the center, menus at the end.
- Keep header bars sparse. A small number of controls is a requirement, not a preference.
- Leave blank draggable space in the header bar.
- Header bar content should change with context when views or modes change.
- Add tooltips to primary window header bar controls.

Button rules inside header bars:

- Prefer icon-only buttons or icon+label buttons.
- Avoid text-only buttons in primary window header bars.
- Avoid linked, suggested, and destructive button styles in primary header bars unless the pattern specifically calls for them.
- Use spacing or `.spacer` separators to group controls instead of linked button styling.

## Menus

- Use a primary menu for app-wide items.
- Use a secondary menu only for view-specific actions when needed.
- Use the `open-menu-symbolic` icon for primary menu buttons.
- Label the menu button accessibly as `Main Menu`.
- Keep menus compact and structured:
  - 3 to 12 items is the target size for a menu;
  - group related items;
  - do not mix actions, check items, and radio items inside the same group;
  - do not nest submenus.
- Primary menus should end with the standard app-wide items when relevant:
  - `Preferences`
  - `Keyboard Shortcuts`
  - `Help`
  - `About <App>`
- Do not put `Close` or `Quit` in the primary menu.

## Buttons and Actions

- Outside header bars, use either an icon or a label on a button, not both.
- Button labels should be short imperative verbs: `Save`, `Install`, `Restore`, `Retry`.
- Disable invalid actions instead of letting users click into predictable errors.
- In the same window, keep adjacent buttons visually aligned and usually the same width.
- Use toggle buttons only for obvious binary or compact mode choices.
- Use at most one emphasized action per view:
  - one `.suggested-action` button for the main affirmative step, or
  - one `.destructive-action` button for the risky action.
- Never style several buttons as equally "primary".

Practical agent rule:

- If a screen has `Cancel`, `Apply`, `Delete`, and `Save`, only one may visually dominate.

## Layout, Padding, and Spacing

Use a restrained spacing scale derived from libadwaita examples:

- `6px` for tight gaps inside toolbars and compact control groups.
- `12px` for side padding and ordinary intra-section spacing.
- `24px` for major section spacing and top/bottom breathing room.

Good defaults:

- Wrap settings and forms in an `AdwClamp` or equivalent constrained content width.
- Use `12px` horizontal margins for standard content columns.
- Use `24px` top/bottom margins for full-page content blocks.
- Use `24px` spacing between major sections.
- Use boxed lists for small static settings groups.
- In boxed list rows, prefer one control, maximum two.

Visual rhythm rules:

- Align to a single content column whenever possible.
- Prefer vertical stacking over dense horizontal packing.
- Break long forms into titled sections instead of one uninterrupted wall.
- Use whitespace, not borders everywhere, to create structure.

Cross-desktop inference:

- Conservative spacing survives theme changes better than ultra-tight or overly airy layouts. LXQt users may run GTK themes that differ from Adwaita/Yaru, so proportion matters more than ornamental framing.

## Typography and Labels

- Use the system font. Do not force a custom application font for normal UI.
- Use standard GTK/libadwaita text styles instead of hard-coded font sizes.
- Minimize the number of weights and sizes in one view.
- Use smaller/lighter text for supporting information and heavier text only where emphasis is needed.
- Avoid all caps.
- Avoid text on textured or graphical backgrounds.
- Keep UI copy short, neutral, and literal.

Good hierarchy:

- page title or section heading;
- body label;
- caption/subtitle for supporting context.

Bad hierarchy:

- five font sizes;
- multiple accent colors;
- decorative hero text in ordinary app screens.

## Icons

- Prefer symbolic GTK icons for normal controls, menus, lists, and header bars.
- Reserve full-color artwork for app icons or deliberate focal visuals.
- Do not mix several icon styles in the same surface.
- Prefer standard symbolic icon names when a common metaphor already exists.

## Colors and Theming

- Default to the standard light style unless the app is primarily for rich visual media.
- Follow the system light/dark preference.
- If the app exposes an appearance switcher, offer `System`, `Light`, and `Dark`.
- Let libadwaita follow the system accent color by default.
- Use CSS variables and style classes instead of hardcoded colors.
- Never rely on color alone to communicate meaning.
- Test custom styling in dark mode and high contrast.

Strong recommendation:

- Do not invent a brand palette for core app chrome.
- Keep brand color usage small and purposeful: one accent, one destructive color, otherwise theme-native neutrals.

Cross-desktop inference:

- This is the main rule that keeps GTK apps looking respectable in LXQt. If you hardcode Adwaita-specific backgrounds, blue accents, low-contrast grays, or custom shadows, the app will age badly outside GNOME.

## Lists, Preferences, and Forms

- Use boxed lists for preferences, short settings groups, and short structured content.
- If a row opens a subpage, end it with `go-next-symbolic`.
- If a row contains a control, clicking the row background should trigger that control.
- Differentiate title, subtitle, and metadata with text weight/size/color, not with extra decoration.
- Use symbolic icons sparingly in rows so they support scanning without dominating the list.
- Keep preferences pages boring in a good way: orderly, sectioned, predictable.

## Dialogs, Errors, and Feedback

- Use dialogs only when the user must stop and make a decision.
- Prefer undo via toast for destructive actions when possible.
- If undo is impossible, use a confirmation dialog for destructive actions.
- Avoid routine error dialogs for simple non-critical failures; use a toast or inline message instead.
- Keep alert dialogs to 1 to 3 buttons.

Practical agent rule:

- If the user can recover after the action, prefer `Delete` + toast with undo.
- If the action is irreversible, confirm before proceeding.

## Adaptive Behavior

- Design from the narrow layout first, then scale up.
- Use libadwaita adaptive containers such as `AdwNavigationSplitView`, `AdwOverlaySplitView`, and `AdwBreakpoint`.
- Collapse sidebars and utility panes on narrow widths instead of squeezing everything tighter.
- Constrain long-form content width with `AdwClamp`.
- Avoid many simultaneous panes; they do not adapt well.

Useful practical thresholds from libadwaita examples:

- around `400sp` is a common breakpoint for collapsing split views;
- around `550sp` is used for switching some wide navigation patterns;
- `12px`/`24px` content margins and section spacing are common example values.

## Accessibility

- Every important control needs a descriptive accessible name.
- Keyboard navigation must reach every interactive element.
- Large text mode must not break layout.
- High contrast must remain legible.
- Screen readers should announce all controls meaningfully.
- Do not use flashing/blinking elements.
- Do not depend on color alone for state or status.

Agent acceptance test:

1. Can the whole window be used with keyboard only?
2. Is every icon-only button labeled accessibly?
3. Does the UI still read clearly in dark mode and high contrast?
4. Do long labels and larger fonts still fit?

## Ubuntu GNOME and LXQt Compatibility Rules

These are portability rules inferred from the sources, especially LXQt theming guidance.

### For Ubuntu GNOME

- Expect Yaru-like GTK styling derived from Adwaita.
- libadwaita conventions will generally look correct out of the box.
- Respect the system accent and appearance preferences.

### For LXQt

- Assume the desktop may use a different GTK theme from GNOME.
- Assume Qt chrome around your GTK app may look different from your internal widget styling.
- Do not rely on ultra-subtle Adwaita-only contrast decisions.
- Do not assume custom CSD ornamentation or custom CSS will blend with the desktop theme.
- Prefer simple surfaces, native widgets, symbolic icons, and clear spacing.

In short:

- On GNOME, elegance comes from following the platform closely.
- On LXQt, elegance comes from restraint and theme tolerance.

## Anti-Patterns for AI Agents

Do not generate GTK UIs that do any of the following:

- hardcode a full custom palette for window backgrounds, cards, and controls;
- pack many controls into the header bar;
- place multiple bright call-to-action buttons in one view;
- use text-only header bar buttons when icons would do;
- mix card grids, boxed lists, heavy borders, and random separators in one screen;
- use more than one navigation metaphor at once unless clearly justified;
- create giant empty windows with tiny content islands;
- use decorative gradients, blurred backdrops, or textured backgrounds for ordinary app screens;
- depend on hover-only affordances;
- hide essential actions inside nested submenus;
- force custom fonts or overly stylized typography;
- treat preferences screens like marketing pages.

## Default Recipe for a Classy GTK App

If an AI agent has no stronger product-specific reason, build screens like this:

1. Use `AdwApplication` + `AdwApplicationWindow`.
2. Add a clean `AdwHeaderBar` with:
   - 1 to 3 meaningful controls;
   - a title or view switcher;
   - a primary menu at the end.
3. Put page content in a single clamped column or a collapsible sidebar/content split.
4. Structure settings as titled sections with boxed lists.
5. Use symbolic icons, short labels, one primary action, and minimal custom CSS.
6. Respect system light/dark/high-contrast/accent settings.
7. Test at narrow width before considering the design complete.

## Prompt Snippet for Other Agents

Use this when asking an AI agent to design or implement a GTK screen:

> Build this as a GTK 4 + libadwaita interface that follows GNOME HIG closely, uses native widgets and symbolic icons, keeps header bars sparse, uses a 6/12/24 spacing rhythm, prefers boxed lists and clamped content widths, supports light/dark/high-contrast cleanly, and avoids hardcoded colors or heavy custom CSS so it still looks disciplined under Ubuntu GNOME and LXQt.

## Sources

Primary GTK/GNOME sources:

- GNOME HIG overview: <https://developer.gnome.org/hig/index.html>
- Windows: <https://developer.gnome.org/hig/patterns/containers/windows.html>
- Header bars: <https://developer.gnome.org/hig/patterns/containers/header-bars.html>
- Buttons: <https://developer.gnome.org/hig/patterns/controls/buttons.html>
- Menus: <https://developer.gnome.org/hig/patterns/controls/menus.html>
- Boxed lists: <https://developer.gnome.org/hig/patterns/containers/boxed-lists.html>
- Dialogs: <https://developer.gnome.org/hig/patterns/feedback/dialogs.html>
- UI styling: <https://developer.gnome.org/hig/guidelines/ui-styling.html>
- Typography: <https://developer.gnome.org/hig/guidelines/typography.html>
- Accessibility: <https://developer.gnome.org/hig/guidelines/accessibility.html>
- UI icons: <https://developer.gnome.org/hig/guidelines/ui-icons.html>
- Scaling and adaptiveness: <https://developer.gnome.org/hig/guidelines/adaptive.html>
- libadwaita style classes: <https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1.6/style-classes.html>
- libadwaita adaptive layouts: <https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1.7/adaptive-layouts.html>
- libadwaita styles and appearance: <https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/styles-and-appearance.html>

Cross-desktop context:

- Ubuntu Yaru theme README: <https://raw.githubusercontent.com/ubuntu/yaru/master/README.md>
- LXQt theming wiki: <https://lxqt-project.org/wiki/Theming.html>

## Notes on Inference

The following recommendations are informed inferences rather than direct quotes from one source:

- the `6 / 12 / 24` spacing rhythm as a general default system;
- the recommendation to keep brand styling out of core app chrome;
- the portability rules specifically aimed at LXQt theme tolerance;
- the summary principle: follow GNOME closely for beauty, and avoid theme-hostile customization for cross-desktop durability.

These inferences are based on the documented libadwaita examples, GNOME HIG conventions, Ubuntu's Adwaita-derived Yaru theme, and LXQt's explicit guidance that consistent cross-toolkit appearance depends on theme alignment rather than app-level custom restyling.
