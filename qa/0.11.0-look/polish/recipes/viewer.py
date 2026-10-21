"""Shoot the viewer page while the agent acts. Usage: python3 viewer.py PREFIX"""
import json, sys, threading, time
from playwright.sync_api import sync_playwright
import mcp

PREFIX = sys.argv[1] if len(sys.argv) > 1 else "v"
ORIGIN = mcp.URL.rsplit("/mcp", 1)[0]
URL = ORIGIN + "/#" + mcp.TOKEN


def refs():
    mcp.text("browser", {"action": "text"})
    raw = mcp.text("browser", {"action": "eval", "js":
        "JSON.stringify(Object.fromEntries([...document.querySelectorAll('input,button')]"
        ".map(e=>[e.id,e.dataset.hotlineRef])))"})
    return json.loads(json.loads(raw))


with sync_playwright() as p:
    b = p.chromium.launch()
    page = b.new_page(viewport={"width": 1440, "height": 900}, device_scale_factor=2)
    page.goto(URL)
    page.wait_for_timeout(2500)
    page.screenshot(path=str(mcp.OUT / f"{PREFIX}-watching.png"))

    r = refs()
    # The agent points at the button: the viewer draws its cursor and intent.
    mcp.text("browser", {"action": "hover", "ref": r["go"]})
    page.wait_for_timeout(300)
    mcp.text("browser", {"action": "click_ref", "ref": r["go"]})
    page.wait_for_timeout(350)
    page.screenshot(path=str(mcp.OUT / f"{PREFIX}-agent-click.png"))

    # A person takes over.
    page.get_by_role("button", name="Take control").click()
    page.mouse.move(700, 450)
    page.wait_for_timeout(800)
    page.screenshot(path=str(mcp.OUT / f"{PREFIX}-driving.png"))
    page.get_by_role("button", name="Give it back").click()
    page.wait_for_timeout(500)

    phone = b.new_page(viewport={"width": 390, "height": 844}, device_scale_factor=3)
    phone.goto(URL)
    phone.wait_for_timeout(2500)
    phone.screenshot(path=str(mcp.OUT / f"{PREFIX}-phone.png"))

    bad = b.new_page(viewport={"width": 1440, "height": 900})
    bad.goto(ORIGIN + "/")
    bad.wait_for_timeout(2500)
    bad.screenshot(path=str(mcp.OUT / f"{PREFIX}-nokey.png"))
    b.close()
print("ok")
