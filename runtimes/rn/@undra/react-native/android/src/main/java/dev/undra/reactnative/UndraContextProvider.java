package dev.undra.reactnative;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;

/**
 * Hands the application context to {@link UndraPlatform} when the app's process starts, before
 * {@code Application.onCreate}: the start-up hook AndroidX Startup uses, declared in this library's manifest, so an app
 * installs nothing. It serves no data. An app that removes it ({@code tools:node="remove"}) calls
 * {@link UndraPlatform#install} itself.
 */
public final class UndraContextProvider extends ContentProvider {
    @Override
    public boolean onCreate() {
        if (getContext() != null) UndraPlatform.install(getContext());
        return true;
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] selectionArgs, String sortOrder) {
        return null;
    }

    @Override
    public String getType(Uri uri) {
        return null;
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        return null;
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
        return 0;
    }
}
