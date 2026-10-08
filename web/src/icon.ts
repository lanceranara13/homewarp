/** How many pixels a side an icon is kept at: twice the largest it is shown at, for a sharp screen. */
const SIDE = 128
/** More than any picture worth choosing, and little enough to read into the page whole. */
const LARGEST = 16 * 1024 * 1024

/**
 * Makes a server's icon of a picture someone chose: the square in its middle,
 * as a small PNG. So any kind of picture this browser can show will do, and
 * Homewarp is only ever sent, and only ever keeps, the one kind.
 */
export async function asIcon(picture: Blob): Promise<Blob> {
  const unreadable = new Error('That is not a picture this browser can read.')
  if (picture.size > LARGEST) throw new Error('A picture for an icon is 16 MB at the most.')
  // Read into an address that holds the picture itself. The content policy lets the page show
  // those, and not the other kind of address a page can make for a file.
  const address = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result))
    reader.onerror = () => reject(unreadable)
    reader.readAsDataURL(picture)
  })
  const shown = new Image()
  shown.src = address
  try {
    await shown.decode()
  } catch {
    throw unreadable
  }
  // A drawing that names no size of its own is taken to be square.
  const width = shown.naturalWidth || SIDE
  const height = shown.naturalHeight || SIDE
  const side = Math.min(width, height)
  const canvas = document.createElement('canvas')
  canvas.width = SIDE
  canvas.height = SIDE
  const pen = canvas.getContext('2d')
  if (!pen) throw unreadable
  pen.imageSmoothingQuality = 'high'
  pen.drawImage(shown, (width - side) / 2, (height - side) / 2, side, side, 0, 0, SIDE, SIDE)
  return new Promise((resolve, reject) => {
    try {
      canvas.toBlob((png) => (png ? resolve(png) : reject(unreadable)), 'image/png')
    } catch {
      // A drawing that brings in something from elsewhere is one a browser will not hand back.
      reject(unreadable)
    }
  })
}
