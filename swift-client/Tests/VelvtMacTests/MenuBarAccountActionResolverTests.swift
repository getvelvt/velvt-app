import XCTest
@testable import VelvtMac

final class MenuBarAccountActionResolverTests: XCTestCase {
    func testLoggedOutAccountShowsSignInAndSignUpActions() {
        XCTAssertEqual(
            MenuBarAccountActionResolver.actions(for: .loggedOut),
            [.authenticate(.logIn), .authenticate(.signUp)]
        )
    }

    func testLoggedInAccountShowsLogOutAndDeleteActions() {
        XCTAssertEqual(
            MenuBarAccountActionResolver.actions(for: .loggedIn(userId: "user")),
            [.logOut, .deleteAccount]
        )
    }

    func testTransientAccountStatesExposeNoActions() {
        XCTAssertEqual(MenuBarAccountActionResolver.actions(for: .loggingIn), [])
        XCTAssertEqual(MenuBarAccountActionResolver.actions(for: .loggingOut), [])
        XCTAssertEqual(MenuBarAccountActionResolver.actions(for: .pendingErasure), [])
    }
}
