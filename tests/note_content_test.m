#define APPLE_CLI_TESTING 1
#include "../helpers/notes-private-injected/AppleNotesPrivateInjected.m"

@interface TestNote : NSObject
@property NSString *title;
@property NSString *body;
@end
@implementation TestNote
- (void)setScriptingBody:(NSString *)body {
    self.body = body;
    self.title = @"Importer replaced title";
}
@end

int main(void) {
    @autoreleasepool {
        TestNote *note = [TestNote new];
        note.title = @"Stable title";
        NSString *expected = applyNoteContent(note, @"<div>Different first paragraph</div>", nil);
        NSCAssert([note.title isEqualToString:@"Stable title"], @"Content updates must preserve title after import");
        NSCAssert([expected isEqualToString:note.title], @"Expected title must match preserved title");
        applyNoteContent(note, @"<div>Body</div>", @"Explicit title");
        NSCAssert([note.title isEqualToString:@"Explicit title"], @"Explicit titles must be applied after import");
        applyNoteContent(note, @"", nil);
        NSCAssert([note.body isEqualToString:@""], @"Empty content must clear the body");
        NSCAssert([note.title isEqualToString:@"Explicit title"], @"Clearing content must preserve title");
        applyNoteContent(note, nil, @"Title only");
        NSCAssert([note.body isEqualToString:@""], @"Title-only updates must not import a body");
        NSCAssert([note.title isEqualToString:@"Title only"], @"Title-only update failed");
        puts("Native title/content ordering: 7 assertions passed (no Notes access)");
    }
}
