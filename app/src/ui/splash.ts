// The startup splash: covers the map while the galaxy loads in, with a line
// from Captain Picard picked at random.

interface Quote {
  text: string;
  /** The episode or film it was said in. */
  from: string;
}

const SERIES = "Star Trek: The Next Generation";
const tng = (episode: string) => `${SERIES}, “${episode}”`;

const QUOTES: Quote[] = [
  { text: "Let’s see what’s out there.", from: tng("Encounter at Farpoint") },
  { text: "If we’re going to be damned, let’s be damned for what we really are.", from: tng("Encounter at Farpoint") },
  { text: "Shut up, Wesley!", from: tng("Datalore") },
  { text: "Things are only impossible until they’re not.", from: tng("When the Bough Breaks") },
  { text: "No being is so important that he can usurp the rights of another.", from: tng("The Schizoid Man") },
  { text: "Starfleet was founded to seek out new life. Well, there it sits!", from: tng("The Measure of a Man") },
  {
    text: "Open your mind to the past. Art, history, philosophy. And all this may mean something.",
    from: tng("Samaritan Snare"),
  },
  {
    text: "It is possible to commit no mistakes and still lose. That is not a weakness; that is life.",
    from: tng("Peak Performance"),
  },
  { text: "A matter of internal security: the age-old cry of the oppressor.", from: tng("The Hunted") },
  { text: "There are times, sir, when men of good conscience cannot blindly follow orders.", from: tng("The Offspring") },
  {
    text:
      "With the first link, the chain is forged. The first speech censured, the first thought forbidden, " +
      "the first freedom denied, chains us all irrevocably.",
    from: tng("The Drumhead"),
  },
  {
    text:
      "Villains who twirl their moustaches are easy to spot. Those who clothe themselves in good deeds " +
      "are well camouflaged.",
    from: tng("The Drumhead"),
  },
  {
    text: "I’d be delighted to offer any advice I have on understanding women. When I have some, I’ll let you know.",
    from: tng("In Theory"),
  },
  { text: "In my experience, communication is a matter of patience, imagination.", from: tng("Darmok") },
  { text: "The first duty of every Starfleet officer is to the truth.", from: tng("The First Duty") },
  {
    text: "Seize the time, Meribor. Live now. Make now always the most precious time. Now will never come again.",
    from: tng("The Inner Light"),
  },
  { text: "There are four lights!", from: tng("Chain of Command, Part II") },
  { text: "I should have done this a long time ago.", from: tng("All Good Things…") },
  { text: "Five-card stud, nothing wild. And the sky’s the limit.", from: tng("All Good Things…") },
  { text: "Tea. Earl Grey. Hot.", from: SERIES },
  { text: "Make it so.", from: SERIES },
  { text: "I rather believe that time is a companion who goes with us on the journey.", from: "Star Trek Generations" },
  { text: "What we leave behind is not as important as how we’ve lived.", from: "Star Trek Generations" },
  { text: "The line must be drawn here! This far, no further!", from: "Star Trek: First Contact" },
  { text: "We work to better ourselves and the rest of humanity.", from: "Star Trek: First Contact" },
];

/** Where the last quote shown is remembered, so two starts in a row differ. */
const LAST_KEY = "splash.lastQuote";
/** The splash lifts after this long whatever is still loading. */
const MAX_SECONDS = 12;
/** Must match the opacity transition on #splash. */
const FADE_MS = 600;

function pickQuote(): Quote {
  let last = -1;
  try {
    last = Number(localStorage.getItem(LAST_KEY) ?? -1);
  } catch {
    // No storage: any quote will do.
  }
  let i = Math.floor(Math.random() * QUOTES.length);
  if (i === last) i = (i + 1 + Math.floor(Math.random() * (QUOTES.length - 1))) % QUOTES.length;
  try {
    localStorage.setItem(LAST_KEY, String(i));
  } catch {
    // Not remembered, then.
  }
  return QUOTES[i];
}

/** Long enough to read the quote: the splash stays at least this long (seconds). */
function readingTime(quote: Quote): number {
  const words = quote.text.split(/\s+/).length;
  return Math.min(6, Math.max(2.5, 1.5 + 0.18 * words));
}

export class Splash {
  private loaded = false;
  private read = false;
  private gone = false;
  private readonly onKey = (e: KeyboardEvent) => {
    // Shortcuts (the dev tools, say) aren't a request to get on with it.
    if (e.ctrlKey || e.altKey || e.metaKey || e.key === "Shift") return;
    this.dismiss();
  };

  constructor(private readonly el: HTMLElement) {
    const quote = pickQuote();
    el.querySelector("blockquote")!.textContent = `“${quote.text}”`;
    el.querySelector(".splash-from")!.textContent = quote.from;
    // Shown once its typeface is in, so it doesn't appear in one face and jump to another.
    const lit = () => el.classList.add("lit");
    void Promise.race([document.fonts.load('500 32px "Antonio"'), new Promise((r) => window.setTimeout(r, 400))]).then(lit, lit);

    window.setTimeout(() => {
      this.read = true;
      this.settle();
    }, readingTime(quote) * 1000);
    window.setTimeout(() => this.dismiss(), MAX_SECONDS * 1000);
    el.addEventListener("pointerdown", () => this.dismiss());
    window.addEventListener("keydown", this.onKey);
  }

  /** What the splash covers is ready to be seen: it lifts once the quote has had its time. */
  done(): void {
    this.loaded = true;
    this.settle();
  }

  /** Lifts the splash now (a click or a key press: the reader has had enough). */
  dismiss(): void {
    if (this.gone) return;
    this.gone = true;
    window.removeEventListener("keydown", this.onKey);
    this.el.classList.add("leaving");
    window.setTimeout(() => this.el.remove(), FADE_MS);
  }

  private settle(): void {
    if (this.loaded && this.read) this.dismiss();
  }
}
