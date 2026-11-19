const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/* ---------- nav on small screens ---------- */
const nav = document.getElementById("nav");
const toggle = document.getElementById("nav-toggle");
if (toggle && nav) {
  const setOpen = (open) => {
    nav.dataset.open = String(open);
    toggle.setAttribute("aria-expanded", String(open));
    toggle.setAttribute("aria-label", open ? "Fermer le menu" : "Ouvrir le menu");
    document.body.style.overflow = open ? "hidden" : "";
  };
  toggle.addEventListener("click", () => setOpen(nav.dataset.open !== "true"));
  nav.querySelectorAll(".nav-links a").forEach((a) => a.addEventListener("click", () => setOpen(false)));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && nav.dataset.open === "true") setOpen(false);
  });
  window.addEventListener("resize", () => {
    if (window.innerWidth > 860 && nav.dataset.open === "true") setOpen(false);
  });
}

/* ---------- reveal on scroll ---------- */
const revealTargets = document.querySelectorAll(
  ".band-head, .install-grid > *, .grid-3 > *, .how > li, .price-grid > *, .honest > li, .faq > details, .reveal-band, .final > *"
);
revealTargets.forEach((el, i) => {
  el.setAttribute("data-reveal", "");
  el.style.transitionDelay = `${Math.min(i % 4, 3) * 60}ms`;
});

if ("IntersectionObserver" in window && !reduce) {
  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          entry.target.classList.add("in");
          io.unobserve(entry.target);
        }
      });
    },
    { rootMargin: "0px 0px -12% 0px", threshold: 0.08 }
  );
  revealTargets.forEach((el) => io.observe(el));
  // What is already on screen when the page loads shows at once, without waiting for a scroll.
  requestAnimationFrame(() => {
    revealTargets.forEach((el) => {
      if (el.getBoundingClientRect().top < window.innerHeight * 0.95) el.classList.add("in");
    });
  });
} else {
  revealTargets.forEach((el) => el.classList.add("in"));
}

/* ---------- the tagline, word by word ---------- */
const tagline = document.getElementById("tagline");
if (tagline) {
  const text = tagline.textContent.trim().replace(/\s+/g, " ");
  tagline.textContent = "";
  const words = text.split(" ").map((word) => {
    const span = document.createElement("span");
    span.className = "w";
    span.textContent = word + " ";
    tagline.appendChild(span);
    return span;
  });
  if (reduce || !("IntersectionObserver" in window)) {
    words.forEach((w) => w.classList.add("on"));
  } else {
    const wordIO = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          const i = words.indexOf(entry.target);
          words.slice(0, i + 1).forEach((w, j) => {
            if (j >= i - 2) setTimeout(() => w.classList.add("on"), (j - (i - 2)) * 90);
            else w.classList.add("on");
          });
          wordIO.unobserve(entry.target);
        });
      },
      { rootMargin: "0px 0px -35% 0px" }
    );
    words.forEach((w) => wordIO.observe(w));
  }
}

/* ---------- screenshots carousel ---------- */
const track = document.getElementById("track");
const dots = document.getElementById("dots");
const prev = document.getElementById("prev");
const next = document.getElementById("next");

if (track && dots) {
  const slides = Array.from(track.querySelectorAll(".slide"));
  const buttons = slides.map((slide, i) => {
    const b = document.createElement("button");
    b.className = "dot";
    b.type = "button";
    b.setAttribute("role", "tab");
    b.setAttribute("aria-selected", i === 0 ? "true" : "false");
    b.textContent = slide.dataset.name || `Capture ${i + 1}`;
    b.addEventListener("click", () => go(i));
    dots.appendChild(b);
    return b;
  });

  const go = (i) => {
    const slide = slides[Math.max(0, Math.min(slides.length - 1, i))];
    track.scrollTo({ left: slide.offsetLeft - track.offsetLeft, behavior: reduce ? "auto" : "smooth" });
  };

  const current = () => {
    const center = track.scrollLeft + track.clientWidth / 2;
    let best = 0;
    let bestDistance = Infinity;
    slides.forEach((slide, i) => {
      const mid = slide.offsetLeft - track.offsetLeft + slide.offsetWidth / 2;
      const d = Math.abs(mid - center);
      if (d < bestDistance) {
        bestDistance = d;
        best = i;
      }
    });
    return best;
  };

  let frame = 0;
  track.addEventListener(
    "scroll",
    () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        const i = current();
        buttons.forEach((b, j) => b.setAttribute("aria-selected", String(j === i)));
      });
    },
    { passive: true }
  );

  prev?.addEventListener("click", () => go(current() - 1));
  next?.addEventListener("click", () => go(current() + 1));
  track.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight") go(current() + 1);
    if (e.key === "ArrowLeft") go(current() - 1);
  });
}

/* ---------- copy to clipboard ---------- */
document.querySelectorAll(".copy").forEach((button) => {
  button.addEventListener("click", async () => {
    const target = document.getElementById(button.dataset.copy);
    if (!target) return;
    try {
      await navigator.clipboard.writeText(target.textContent.trim());
      button.textContent = "Copié";
    } catch {
      button.textContent = "Sélectionnez la commande";
    }
    setTimeout(() => (button.textContent = "Copier"), 2200);
  });
});
