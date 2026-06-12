package com.example.service;

import com.example.model.User;

public class NotificationService {
    public void sendWelcomeEmail(User user) {
        System.out.println("Welcome " + user.getName() + " to " + user.getEmail());
    }

    public void sendDeletionNotice(User user) {
        System.out.println("Account deleted for " + user.getName());
    }
}
