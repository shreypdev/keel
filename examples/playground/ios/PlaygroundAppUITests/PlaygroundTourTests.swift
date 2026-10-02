import XCTest

/// Taps through the five screens of the playground against the real core and checks what the stores
/// show. Each test starts on its own tab (`-tab`), like a screenshot run would.
///
/// When the environment variable `TEST_RUNNER_PROOF_DIR` is set, every test also saves the screen
/// it ends on as `<dir>/ios-<name>.png`.
@MainActor
final class PlaygroundTourTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launch(tab: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-tab", tab]
        app.launch()
        return app
    }

    /// Waits for `element` to exist.
    private func wait(for element: XCUIElement, _ what: String, timeout: TimeInterval = 10) {
        if !element.waitForExistence(timeout: timeout) {
            keepScreenshot(named: "missing-\(what.replacingOccurrences(of: " ", with: "-"))")
            XCTFail("\(what) never appeared")
        }
    }

    /// Waits until the label of `element` is `text`.
    private func wait(for element: XCUIElement, toRead text: String, timeout: TimeInterval = 10) {
        let predicate = NSPredicate(format: "label == %@", text)
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [expectation], timeout: timeout), .completed,
                       "\(element.identifier) reads \"\(element.label)\", not \"\(text)\"")
    }

    /// Waits until the label of `element` ends with `text` (a `LabeledContent` reads "Status, Success").
    private func wait(for element: XCUIElement, toEndWith text: String, timeout: TimeInterval = 10) {
        let predicate = NSPredicate(format: "label ENDSWITH %@", text)
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [expectation], timeout: timeout), .completed,
                       "\(element.identifier) reads \"\(element.label)\", which does not end with \"\(text)\"")
    }

    /// Waits until the label of `element` contains `text`.
    private func wait(for element: XCUIElement, toContain text: String, timeout: TimeInterval = 10) {
        let predicate = NSPredicate(format: "label CONTAINS %@", text)
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [expectation], timeout: timeout), .completed,
                       "\(element.identifier) reads \"\(element.label)\", which does not contain \"\(text)\"")
    }

    /// Flips a switch by tapping the switch itself: its element spans the row, and the middle of
    /// the row is the label, which does not toggle.
    private func flip(_ toggle: XCUIElement) {
        toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.93, dy: 0.5)).tap()
    }

    /// The element with accessibility identifier `id`, whatever its type.
    private func element(_ id: String, in app: XCUIApplication) -> XCUIElement {
        return app.descendants(matching: .any).matching(identifier: id).firstMatch
    }

    /// Keeps the screen as an attachment and, when asked, as a file.
    private func keepScreenshot(named name: String) {
        let screenshot = XCUIScreen.main.screenshot()
        let attachment = XCTAttachment(screenshot: screenshot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        if let directory = ProcessInfo.processInfo.environment["PROOF_DIR"] {
            try? screenshot.pngRepresentation.write(to: URL(fileURLWithPath: directory).appendingPathComponent("ios-\(name).png"))
        }
    }

    // MARK: Todos

    func testTodos() {
        let app = launch(tab: "todos")
        let input = app.textFields["todo-input"]
        wait(for: input, "the todo input")
        wait(for: app.staticTexts["remaining"], toRead: "0 left")

        // The core refuses a blank title with its typed error.
        input.tap()
        input.typeText("   ")
        app.buttons["todo-add"].tap()
        wait(for: app.staticTexts["todo-problem"], toRead: "the title cannot be empty")

        for title in ["Buy milk", "Walk the dog", "Write Undra"] {
            input.tap()
            if let current = input.value as? String, current != "What needs doing?", !current.isEmpty {
                input.typeText(XCUIKeyboardKey.delete.rawValue.description)
            }
            input.typeText(title)
            app.buttons["todo-add"].tap()
        }
        wait(for: app.staticTexts["remaining"], toRead: "3 left")
        XCTAssertEqual(app.buttons.matching(identifier: "todo-row").count, 3)

        // Finish one; `remaining` and the filtered list are computed in the core.
        app.buttons.matching(identifier: "todo-row").element(boundBy: 1).tap()
        wait(for: app.staticTexts["remaining"], toRead: "2 left")
        app.segmentedControls["todo-filter"].buttons["Done"].tap()
        XCTAssertTrue(app.buttons["Walk the dog"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.buttons.matching(identifier: "todo-row").count, 1)
        app.segmentedControls["todo-filter"].buttons["Active"].tap()
        XCTAssertTrue(app.buttons["Buy milk"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.buttons.matching(identifier: "todo-row").count, 2)
        app.segmentedControls["todo-filter"].buttons["All"].tap()

        // Swipe to remove, then clear what is done.
        app.buttons["Write Undra"].swipeLeft()
        app.buttons["Delete"].tap()
        wait(for: app.staticTexts["remaining"], toRead: "1 left")
        app.buttons["todo-clear-done"].tap()
        XCTAssertEqual(app.buttons.matching(identifier: "todo-row").count, 1)
        keepScreenshot(named: "todos-tour")
    }

    // MARK: Counter

    func testCounter() {
        let app = launch(tab: "counter")
        wait(for: app.staticTexts["counter-value"], toRead: "0")
        XCTAssertEqual(app.staticTexts["counter-parity"].label, "even")

        for _ in 0 ..< 3 {
            app.buttons["counter-inc"].tap()
        }
        wait(for: app.staticTexts["counter-value"], toRead: "3")
        wait(for: app.staticTexts["counter-parity"], toRead: "odd")
        wait(for: app.staticTexts["counter-changes"], toRead: "3 changes")
        keepScreenshot(named: "counter-tour")

        app.buttons["counter-dec"].tap()
        wait(for: app.staticTexts["counter-value"], toRead: "2")
        wait(for: app.staticTexts["counter-parity"], toRead: "even")

        app.buttons["counter-reset"].tap()
        wait(for: app.staticTexts["counter-value"], toRead: "0")
        wait(for: app.staticTexts["counter-changes"], toRead: "0 changes")
    }

    // MARK: 10k list

    func testBigList() {
        let app = launch(tab: "biglist")
        wait(for: app.staticTexts["biglist-count"], toRead: "10,000 rows")
        wait(for: element("biglist-row-1", in: app), "the first row")

        app.buttons["biglist-insert-top"].tap()
        wait(for: app.staticTexts["biglist-count"], toRead: "10,001 rows")
        wait(for: element("biglist-row-10001", in: app), "the inserted row")

        app.buttons["biglist-remove"].tap()
        wait(for: app.staticTexts["biglist-count"], toRead: "10,000 rows")

        /// A fresh query each time: the rows whose label says they were updated.
        func updatedRows() -> XCUIElementQuery {
            return app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'biglist-row-' AND label CONTAINS 'Updated'"))
        }
        app.buttons["biglist-update"].tap()
        XCTAssertTrue(updatedRows().firstMatch.waitForExistence(timeout: 5))

        // Ten updates a second on rows that are on screen.
        flip(app.switches["biglist-stream"])
        let deadline = Date().addingTimeInterval(10)
        while updatedRows().count < 5, Date() < deadline {
            Thread.sleep(forTimeInterval: 0.25)
        }
        XCTAssertGreaterThanOrEqual(updatedRows().count, 5, "the stream updated too few visible rows")
        keepScreenshot(named: "biglist-streaming")
        flip(app.switches["biglist-stream"])

        app.buttons["biglist-reset"].tap()
        wait(for: app.staticTexts["biglist-count"], toRead: "10,000 rows")
    }

    // MARK: Remote

    func testRemote() {
        let app = launch(tab: "remote")
        wait(for: app.staticTexts["remote-status"], toEndWith: "Success")
        let rows = app.buttons.matching(identifier: "remote-row")
        XCTAssertEqual(rows.count, 3, "the server's three seeded items")

        // Optimistic add: the row shows at once, and the server's item replaces it.
        let input = app.textFields["remote-input"]
        input.tap()
        input.typeText("Call Mum")
        app.buttons["remote-add"].tap()
        wait(for: app.buttons["Call Mum"], "the new item")
        let served = NSPredicate(format: "count == 4")
        wait(for: XCTNSPredicateExpectation(predicate: served, object: rows), timeout: 10)

        // Completing an item goes to the server too.
        app.buttons["Buy milk"].tap()
        app.buttons["remote-refresh"].tap()
        wait(for: app.staticTexts["remote-status"], toEndWith: "Success")

        // Offline: the creation waits in the core's queue, and is sent when the network returns.
        flip(app.switches["remote-offline"])
        input.tap()
        input.typeText("Write the report")
        app.buttons["remote-add"].tap()
        wait(for: element("remote-row-pending", in: app), "the queued item")
        Thread.sleep(forTimeInterval: 1.5)
        XCTAssertTrue(element("remote-row-pending", in: app).exists, "the queued item must still be waiting")
        keepScreenshot(named: "remote-offline")
        flip(app.switches["remote-offline"])
        let replayed = NSPredicate(format: "count == 5")
        wait(for: XCTNSPredicateExpectation(predicate: replayed, object: rows), timeout: 15)
        XCTAssertFalse(element("remote-row-pending", in: app).exists)
        wait(for: app.staticTexts["remote-status"], toEndWith: "Success")
    }

    // MARK: Notes

    func testNotes() {
        let app = launch(tab: "notes")
        // Opening ran the two migrations (or found them done by an earlier launch).
        wait(for: app.staticTexts["notes-version"], toEndWith: "schema version 2")
        let rows = app.buttons.matching(identifier: "note-row")
        let before = rows.count
        let title = "Note \(Int(Date().timeIntervalSince1970))"
        let input = app.textFields["note-input"]
        input.tap()
        input.typeText(title)
        app.buttons["note-add"].tap()
        wait(for: app.buttons[title], "the new note")
        wait(for: app.staticTexts["notes-count"], toRead: "\(before + 1) notes")
        // Ticking it is an UPDATE in SQLite, then the mirror; the row stays.
        app.buttons[title].tap()
        XCTAssertTrue(app.buttons[title].waitForExistence(timeout: 5))
        keepScreenshot(named: "notes-tour")
        // Swipe to remove: a DELETE, then the row goes.
        app.buttons[title].swipeLeft()
        app.buttons["Delete"].tap()
        wait(for: app.staticTexts["notes-count"], toRead: "\(before) notes")
        XCTAssertFalse(app.buttons[title].exists)
    }

    // MARK: Library

    func testLibrary() {
        let app = launch(tab: "library")
        wait(for: app.staticTexts["library-count"], toRead: "10,000 books")
        // Rows arrive a page at a time: the first one shows its book once its page has come (a placeholder until then).
        wait(for: element("library-row-0", in: app), "the first book")
        XCTAssertTrue(element("library-row-0", in: app).label.contains("Item 1"), "row 0 reads \"\(element("library-row-0", in: app).label)\"")

        // One call each; the core answers with a new length and version, and the visible pages are asked for again.
        app.buttons["library-add"].tap()
        wait(for: app.staticTexts["library-count"], toRead: "10,001 books")
        app.buttons["library-rename"].tap()
        wait(for: element("library-row-0", in: app), toContain: "Renamed 1")
        app.buttons["library-remove"].tap()
        wait(for: app.staticTexts["library-count"], toRead: "10,000 books")

        // The second section is a lazy view of the even rows of a small list; dropping a source row drops its row.
        wait(for: app.staticTexts["library-evens-count"], toRead: "10 even rows of the source")
        app.buttons["library-drop-source"].tap()
        app.buttons["library-drop-source"].tap()
        wait(for: app.staticTexts["library-evens-count"], toRead: "9 even rows of the source")
        keepScreenshot(named: "library-tour")
    }

    // MARK: Feed

    func testFeed() {
        let app = launch(tab: "feed")
        wait(for: app.staticTexts["feed-count"], toRead: "50")
        wait(for: app.staticTexts["feed-status"], toRead: "Success")
        wait(for: element("feed-row-1", in: app), "the first row")

        // Scrolling to the end makes the rows near it ask for the next page, which arrives as 50 more rows.
        let list = app.collectionViews["feed-list"].exists ? app.collectionViews["feed-list"] : app.tables["feed-list"]
        let deadline = Date().addingTimeInterval(30)
        while app.staticTexts["feed-count"].label != "100", Date() < deadline {
            list.swipeUp(velocity: .fast)
        }
        wait(for: app.staticTexts["feed-count"], toRead: "100")
        keepScreenshot(named: "feed-tour")

        // A refetch finds only what changed: the even rows now show revision 1, and the list is as long as before.
        list.swipeDown(velocity: .fast)
        app.buttons["feed-touch"].tap()
        wait(for: app.staticTexts["feed-status"], toRead: "Success")
        wait(for: app.staticTexts["feed-count"], toRead: "100")
        wait(for: element("feed-row-2", in: app), toContain: "v1")

        // The even rows are another parameter of the query: another entry with its own first page.
        app.segmentedControls["feed-filter"].buttons["Even rows"].tap()
        wait(for: app.staticTexts["feed-count"], toRead: "50")
        wait(for: element("feed-row-100", in: app).exists ? element("feed-row-100", in: app) : element("feed-row-2", in: app), "an even row")
    }

    // MARK: Ticker

    func testTicker() {
        let app = launch(tab: "ticker")
        let value = app.staticTexts["ticker-value"]
        wait(for: value, "the counter")
        // The first tick, then one a second without anybody asking: the core polls while this screen watches.
        let first = NSPredicate(format: "label != 'none yet'")
        XCTAssertEqual(XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: first, object: value)], timeout: 10), .completed)
        let start = Int(value.label) ?? 0
        let advanced = NSPredicate(format: "label.integerValue >= %d", start + 2)
        XCTAssertEqual(XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: advanced, object: value)], timeout: 10), .completed,
                       "the counter reads \"\(value.label)\" after the first tick \(start)")

        // This observer asks for a faster poll.
        let mark = Int(value.label) ?? 0
        flip(app.switches["ticker-fast"])
        let faster = NSPredicate(format: "label.integerValue >= %d", mark + 4)
        XCTAssertEqual(XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: faster, object: value)], timeout: 2.5), .completed,
                       "the counter reads \"\(value.label)\" a second and a half after asking for 250 ms (it was \(mark))")
        flip(app.switches["ticker-fast"])

        // A failure the core makes on purpose shows, polling goes on, and the error clears with the next success.
        flip(app.switches["ticker-failing"])
        wait(for: element("ticker-error", in: app), "the error", timeout: 5)
        keepScreenshot(named: "ticker-failing")
        flip(app.switches["ticker-failing"])
        let gone = NSPredicate(format: "exists == false")
        XCTAssertEqual(XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: gone, object: element("ticker-error", in: app))], timeout: 5), .completed)
        wait(for: app.staticTexts["ticker-status"], toRead: "Success")
    }

    private func wait(for expectation: XCTNSPredicateExpectation, timeout: TimeInterval) {
        XCTAssertEqual(XCTWaiter().wait(for: [expectation], timeout: timeout), .completed, "timed out waiting for \(expectation)")
    }

    // MARK: Tabs

    func testTabBarSwitchesScreens() {
        let app = launch(tab: "todos")
        for (id, marker) in [("counter", "counter-value"), ("biglist", "biglist-count"), ("remote", "remote-status"), ("notes", "notes-count"), ("todos", "remaining")] {
            app.tabBars.buttons["tab-\(id)"].tap()
            wait(for: app.staticTexts[marker], "the \(id) screen")
        }
    }
}
