
const FAVICON_SIZE = 48;

let currentFaviconLink: HTMLLinkElement | null = null;

/** The MARFI mark is fixed brand artwork, not recolored by custom themes. */
export function getFaviconUrl(_color: string) {
  return "/app/favicon-32x32.png";
}

/**
 * Update the site's live favicon with a new color, and optionally a notification
 * badge with its own color.
 */
export function updateFavicon(
  faviconColor: string,
  badgeColor?: string,
  hasBadge?: boolean
): void {
  if (currentFaviconLink?.parentNode) {
    currentFaviconLink.parentNode.removeChild(currentFaviconLink);
    currentFaviconLink = null;
  }

  const canvas = document.createElement('canvas');
  const ctx = canvas.getContext('2d');
  if (!ctx) return;

  canvas.width = FAVICON_SIZE;
  canvas.height = FAVICON_SIZE;

  const img = new Image();
  img.src = getFaviconUrl(faviconColor);

  img.onload = () => {
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);

    if (hasBadge) {
      const badgeRadius = 6;
      ctx.beginPath();
      ctx.arc(
        canvas.width - badgeRadius,
        badgeRadius,
        badgeRadius,
        0,
        2 * Math.PI
      );
      ctx.fillStyle = badgeColor || faviconColor;
      ctx.fill();
    }

    const faviconUrl = canvas.toDataURL();

    if (currentFaviconLink?.parentNode) {
      currentFaviconLink.parentNode.removeChild(currentFaviconLink);
    }

    const existingLinks = document.querySelectorAll('link[rel*="icon"]');
    existingLinks.forEach((link) => {
      link.remove();
    });

    // create and add new favicon
    const link = document.createElement('link');
    link.rel = 'icon';
    link.type = 'image/png';
    link.href = faviconUrl;
    document.head.appendChild(link);
    currentFaviconLink = link;

    // update existing shortcut icon if present
    const existingShortcutIcon = document.querySelector(
      'link[rel="shortcut icon"]'
    ) as HTMLLinkElement;
    if (existingShortcutIcon) {
      existingShortcutIcon.href = faviconUrl;
    }
  };
}
